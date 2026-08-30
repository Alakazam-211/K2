pub mod schema;

use parking_lot::ReentrantMutex;
use rusqlite::{params, Connection, Result};
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Process-wide SQLite handle. Populated exactly once by
/// [`init_database`] during app startup and accessed from any thread via
/// [`shared`]. The `Arc<ReentrantMutex<Connection>>` shape means `AppState.db`
/// and every ad-hoc command-handler/HTTP-thread caller can clone the
/// same handle — there is only one physical connection (and therefore
/// only one write lock queue) for the lifetime of the process.
///
/// Rationale: rusqlite connections are not `Sync`, so they must sit
/// behind a mutex. A SINGLE connection is the right call here because
/// WAL mode already serializes writes at the database level — spinning
/// up multiple connections just multiplies the places the `SQLITE_BUSY`
/// error can surface without buying parallelism. Parallel-reader code
/// paths are rare in K2SO (most work is write-heavy: agent sessions,
/// work items, heartbeats). When that changes, swap this for an
/// `r2d2::Pool<SqliteConnectionManager>` — the public API stays the same.
static SHARED: OnceLock<Arc<ReentrantMutex<Connection>>> = OnceLock::new();

/// Open a SQLite connection with K2SO's standard resilience + performance
/// PRAGMAs.
///
/// **Resilience**
/// - WAL mode (set once per database — readers don't block writers).
/// - busy_timeout **500 ms** (was 5000 ms pre-0.32.13; 5 s was masking real
///   contention behind a UI hang. Zed and the rusqlite community both use
///   500 ms.). Waits on contention instead of SQLITE_BUSY-failing immediately.
/// - foreign_keys ON.
///
/// **Performance** (added 0.32.13, all benchmarked in Zed + Spacedrive)
/// - `cache_size = -20000` — 20 MB page cache per connection. Without this
///   SQLite uses the built-in 2 MB default.
/// - `mmap_size = 67108864` — map the first 64 MB of the database file for
///   reads. Cuts read-path syscall count on the common hot queries.
/// - `temp_store = MEMORY` — keep any temp tables / sort buffers in RAM.
///
/// **Only use this for standalone tools or migration scripts.** Runtime
/// code should always access the shared connection via [`shared`] so it
/// isn't racing against the AppState connection for write slots.
pub fn open_with_resilience<P: AsRef<Path>>(path: P) -> Result<Connection> {
    let conn = Connection::open(path)?;
    // Resilience PRAGMAs.
    conn.busy_timeout(std::time::Duration::from_millis(500))?;
    // Each PRAGMA logged-but-not-fatal — the connection is usable without
    // them even if a particular pragma fails on an exotic SQLite build.
    let _ = conn.execute_batch(
        "PRAGMA journal_mode = WAL;\n\
         PRAGMA foreign_keys = ON;\n\
         PRAGMA cache_size = -20000;\n\
         PRAGMA mmap_size = 67108864;\n\
         PRAGMA temp_store = MEMORY;",
    );
    Ok(conn)
}

/// Clone a handle to the process-wide SQLite connection. In production
/// builds this panics (with a diagnostic) if called before
/// [`init_database`] — which would only happen via a programming error,
/// not a user-reachable path. All startup flows call init_database
/// before the first command handler or HTTP endpoint can fire.
///
/// Under `#[cfg(test)]` this lazily initializes to an in-memory SQLite
/// on first call, so unit tests that exercise code paths touching the DB
/// don't need to wire up the full Tauri startup. Production builds do
/// NOT get this lazy-init — missing startup initialization must be a
/// hard error, not a silent fallback to an ephemeral DB.
///
/// Usage pattern:
///   let db = crate::db::shared();
///   let conn = db.lock();
///   conn.execute(...)?;
///
/// The returned `Arc` is cheap to clone but the lock must be acquired
/// before each SQL operation. Hold the lock for the duration of a
/// transaction block, then drop the guard to release the write queue.
pub fn shared() -> Arc<ReentrantMutex<Connection>> {
    if let Some(handle) = SHARED.get() {
        return handle.clone();
    }
    #[cfg(any(test, feature = "test-util"))]
    {
        return init_for_tests();
    }
    #[cfg(not(any(test, feature = "test-util")))]
    {
        panic!("db::init_database must run before db::shared()");
    }
}

/// Like [`shared`], but `None` when the process-wide DB has not been
/// initialized (lib tests in downstream crates that never called
/// `init_database`). Callers that can fall back (inject flow) use this
/// instead of panicking.
pub fn try_shared() -> Option<Arc<ReentrantMutex<Connection>>> {
    SHARED.get().cloned()
}

/// Test-only: populate SHARED with an in-memory SQLite that's been
/// through the full migration + seed sequence. Idempotent across test
/// threads because OnceLock::set is atomic — losers drop their handle
/// and clone the winner's.
///
/// Caveat: every unit test in the process shares this one in-memory DB.
/// Tests that expect isolated DB state must either (a) clean up their
/// rows on exit, or (b) use a scratch_project() directory pattern that
/// keeps filesystem state separate even when DB state overlaps.
///
/// Gated on `#[cfg(test)]` OR the `test-util` feature so downstream
/// crates' test binaries can reach it (their cfg(test) doesn't flip
/// cfg(test) here). Production builds compile this out, restoring the
/// invariant that only test contexts can acquire an in-memory DB
/// without first calling `init_database()`.
#[cfg(any(test, feature = "test-util"))]
pub fn init_for_tests() -> Arc<ReentrantMutex<Connection>> {
    if let Some(handle) = SHARED.get() {
        return handle.clone();
    }
    let conn = Connection::open(":memory:")
        .expect("in-memory SQLite open failed");
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .expect("set busy_timeout");
    let _ = conn.execute_batch("PRAGMA foreign_keys = ON;");
    run_migrations(&conn).expect("test migrations");
    seed_agent_presets(&conn).expect("test seed");
    seed_audit_sentinels(&conn).expect("test audit sentinels");
    crate::workspace::handle::backfill_workspace_handles(&conn);
    let handle = Arc::new(ReentrantMutex::new(conn));
    match SHARED.set(handle.clone()) {
        Ok(()) => handle,
        Err(_) => SHARED.get().expect("SHARED populated").clone(),
    }
}

/// Resolve the on-disk SQLite path under a K2 home dir (`~/.k2` in prod).
///
/// Endgame Stage A (prd-k2so-endgame-v1): **prefer `k2.db` if it exists**,
/// else use legacy `k2so.db` (create path for fresh installs until Stage B
/// flips the writer).
///
/// **Both real files:** Stage A used to pick `k2.db` blindly. A stray
/// stub `k2.db` (touch / `sqlite3 ~/.k2/k2.db` / a test) then hid the
/// live `k2so.db` — workspaces and mail looked gone. If both exist as
/// non-symlink files, pick the one with more non-sentinel `projects`
/// rows (size as fallback). Never delete either file. Symlink
/// `k2so.db` → `k2.db` (Stage B) still resolves to `k2.db`.
pub fn resolve_home_db_path(db_dir: &std::path::Path) -> std::path::PathBuf {
    let new = db_dir.join("k2.db");
    let old = db_dir.join("k2so.db");
    if is_real_db_file(&new) && is_real_db_file(&old) {
        let pick = pick_dual_real_db(&new, &old);
        if pick == old {
            eprintln!(
                "[db] BOTH k2.db and k2so.db exist as real files; using k2so.db \
                 (k2.db looks like a stub — a Stage A prefer-k2.db pick would \
                 hide workspaces). Not deleting either file."
            );
        } else {
            eprintln!(
                "[db] BOTH k2.db and k2so.db exist as real files; using k2.db \
                 (it has the live project rows). Not deleting either file."
            );
        }
        return pick;
    }
    if new.exists() {
        new
    } else {
        old
    }
}

fn is_real_db_file(path: &std::path::Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(m) => m.is_file(),
        Err(_) => false,
    }
}

fn file_len(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Count real workspaces. Sentinels `_orphan` / `_broadcast` do not count
/// (a migrated empty `k2.db` still has those two rows).
fn live_project_count(path: &std::path::Path) -> i64 {
    let Ok(conn) = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return 0;
    };
    conn.query_row(
        "SELECT count(*) FROM projects WHERE id NOT IN ('_orphan', '_broadcast')",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

fn pick_dual_real_db(new: &std::path::Path, old: &std::path::Path) -> std::path::PathBuf {
    let cn = live_project_count(new);
    let co = live_project_count(old);
    if co > cn {
        return old.to_path_buf();
    }
    if cn > co {
        return new.to_path_buf();
    }
    let ln = file_len(new);
    let lo = file_len(old);
    // Equal project counts (both stub or unreadable): a tiny k2.db must
    // not hide a large k2so.db.
    if lo > ln.saturating_mul(2) && lo > 1_000_000 {
        return old.to_path_buf();
    }
    if ln > lo.saturating_mul(2) && ln > 1_000_000 {
        return new.to_path_buf();
    }
    old.to_path_buf()
}

/// Open (or create) the K2 database under `~/.k2/`, run all migrations,
/// seed default data, and populate the process-wide [`SHARED`] connection.
/// Returns an `Arc` handle so the caller can store it in `AppState.db`
/// AND the shared static points at the same physical connection.
///
/// Filename: **endgame Stage A** prefers `k2.db` when it already exists;
/// otherwise opens/creates legacy `k2so.db` so a Stage-A-only release does
/// not rewrite on-disk data. Stage B flips the writer / renames (see
/// `prd-k2so-endgame-v1.md`).
///
/// Safe to call exactly once per process. A second call returns the
/// already-initialized handle (tests that reuse the binary hit this).
pub fn init_database() -> Result<Arc<ReentrantMutex<Connection>>> {
    // Fast path for tests that re-invoke the init (or if somewhere in
    // startup accidentally re-initializes): just clone the existing
    // handle rather than opening another connection.
    if let Some(existing) = SHARED.get() {
        return Ok(existing.clone());
    }

    let db_dir = dirs::home_dir()
        .ok_or_else(|| rusqlite::Error::InvalidParameterName("Could not determine home directory".to_string()))?
        .join(".k2");
    std::fs::create_dir_all(&db_dir)
        .map_err(|e| rusqlite::Error::InvalidParameterName(format!("Could not create ~/.k2 directory: {}", e)))?;

    let db_path = resolve_home_db_path(&db_dir);
    let conn = open_with_resilience(&db_path)?;

    // Self-heal: clean orphan rows whose parent `projects` row was
    // deleted under earlier versions where FK enforcement was off
    // or a delete path bypassed CASCADE. Runs BEFORE migrations so
    // 0.37.0's `INSERT INTO workspace_sessions … SELECT … FROM
    // agent_sessions` (which adds a NOT NULL REFERENCES projects(id)
    // FK) doesn't trip on stranded rows. One client's DB had 615
    // such rows across `activity_feed` / `heartbeat_fires` /
    // `agent_sessions`, causing 0.37.0 to crash on launch with
    // "FATAL: Failed to initialize database: FOREIGN KEY constraint
    // failed". The CASCADE rule on every FK declaration says these
    // rows should already be gone — this just finishes the deletion
    // that didn't happen.
    purge_orphan_project_children(&conn)?;

    run_migrations(&conn)?;
    seed_agent_presets(&conn)?;
    seed_audit_sentinels(&conn)?;
    crate::workspace::handle::backfill_workspace_handles(&conn);

    let handle = Arc::new(ReentrantMutex::new(conn));
    // Race-free publish: whoever wins gets their handle stored, losers
    // drop theirs and return the winner's. In practice only one thread
    // calls init_database during startup.
    match SHARED.set(handle.clone()) {
        Ok(()) => Ok(handle),
        Err(_) => Ok(SHARED.get().expect("SHARED just populated").clone()),
    }
}

/// Bootstrap a brand-new database file at `path` with the full migration
/// + seed sequence. Test-only: used by concurrency tests that need
/// multiple `Connection`s sharing real disk state (the in-memory default
/// gives each connection a separate database). Writing on-disk tempfiles
/// makes multi-connection CAS behavior observable.
///
/// Production code must never use this — `init_database()` handles the
/// real startup path and publishes the shared connection.
#[cfg(test)]
pub(crate) fn bootstrap_test_db_at<P: AsRef<Path>>(path: P) -> Result<()> {
    let conn = open_with_resilience(path)?;
    run_migrations(&conn)?;
    seed_agent_presets(&conn)?;
    seed_audit_sentinels(&conn)?;
    crate::workspace::handle::backfill_workspace_handles(&conn);
    Ok(())
}

/// Build a fresh isolated in-memory connection. Test-only. Unlike the
/// shared `init_for_tests()` helper, each call returns its own handle
/// backed by its own `:memory:` database — so tests that assert on
/// specific row counts, migration state, or table contents can't
/// collide with other tests in the same process.
#[cfg(test)]
pub(crate) fn isolated_test_connection() -> Connection {
    let conn = Connection::open(":memory:").expect("open :memory:");
    conn.busy_timeout(std::time::Duration::from_millis(5000))
        .expect("busy_timeout");
    let _ = conn.execute_batch("PRAGMA foreign_keys = ON;");
    run_migrations(&conn).expect("migrations");
    seed_agent_presets(&conn).expect("seed");
    seed_audit_sentinels(&conn).expect("audit sentinels");
    crate::workspace::handle::backfill_workspace_handles(&conn);
    conn
}

/// Self-heal sweep: remove rows in FK-bearing project-child tables
/// whose `project_id` no longer exists in `projects`. Runs before
/// migrations so 0.37.0's table-rebuild migrations (which add
/// `REFERENCES projects(id)` constraints) don't fail with
/// "FOREIGN KEY constraint failed" on databases that accumulated
/// orphans under earlier versions.
///
/// FK constraints are toggled OFF for the duration of the DELETE
/// to avoid triggering cascading checks while we're cleaning up;
/// re-enabled afterwards. The deletes themselves are intentionally
/// idempotent — every CASCADE rule on these tables says these rows
/// should already be gone, so this just finishes the deletion that
/// didn't happen in earlier versions where FK enforcement was off
/// per-connection or a delete path bypassed CASCADE.
///
/// Tables we check (every FK to projects.id we ship):
/// - `agent_sessions`           (pre-0.39 → renamed to workspace_sessions in 0039)
/// - `agent_heartbeats`         (pre-0.40 → renamed to workspace_heartbeats in 0040)
/// - `workspace_sessions`       (post-0.39, but the table name was
///                               also used pre-0.38 for tab layouts;
///                               we check it conditionally)
/// - `heartbeat_fires`
/// - `activity_feed`
/// - `workspace_layouts`        (renamed from old workspace_sessions in 0038)
///
/// Conditional `IF EXISTS`-style guards via sqlite_master so the
/// sweep is safe whether it runs pre-0.37.0 (legacy tables exist)
/// or post-0.37.0 (renamed tables exist) or in any partially-migrated
/// state. Returns `Ok(())` if the projects table doesn't exist yet
/// (fresh DB, nothing to heal).
pub(crate) fn purge_orphan_project_children(conn: &Connection) -> Result<()> {
    // Fresh DB — no `projects` table yet, no orphans possible.
    let projects_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='projects'",
        [],
        |r| r.get(0),
    )?;
    if projects_exists == 0 {
        return Ok(());
    }

    // Tables we know carry a `project_id` FK to projects(id) at
    // some point in the schema's history. Each entry is checked
    // for existence before the DELETE so we don't fail on
    // partial-migration state.
    let candidate_tables = [
        "agent_sessions",
        "agent_heartbeats",
        "workspace_sessions",
        "workspace_heartbeats",
        "heartbeat_fires",
        "activity_feed",
        "workspace_layouts",
        "tab_titles",
    ];

    // FK enforcement off for the cleanup so we don't trip
    // intermediate constraints on tables that still reference each
    // other through the orphan chain. Re-enabled before we exit.
    let _ = conn.execute_batch("PRAGMA foreign_keys = OFF;");

    let mut total_purged = 0i64;
    for table in &candidate_tables {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |r| r.get(0),
        )?;
        if exists == 0 {
            continue;
        }
        // Only delete from tables that actually have a `project_id`
        // column. Older variants (e.g., the original 0009-vintage
        // workspace_sessions before the 0038 rename) had different
        // shapes; we shouldn't touch those.
        let has_col: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name='project_id'",
                table
            ),
            [],
            |r| r.get(0),
        )?;
        if has_col == 0 {
            continue;
        }
        let stmt = format!(
            "DELETE FROM {} WHERE project_id NOT IN (SELECT id FROM projects)",
            table
        );
        let n = conn.execute(&stmt, [])?;
        if n > 0 {
            total_purged += n as i64;
            crate::log_debug!(
                "[db/self-heal] purged {n} orphan rows from {table} \
                 (project_id no longer exists in projects)"
            );
        }
    }

    let _ = conn.execute_batch("PRAGMA foreign_keys = ON;");

    if total_purged > 0 {
        crate::log_debug!(
            "[db/self-heal] total orphan rows purged across all FK-bearing tables: {}",
            total_purged
        );
    }
    Ok(())
}

/// Simple migration runner using a _migrations table to track applied migrations.
pub(crate) fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            applied_at INTEGER NOT NULL DEFAULT (unixepoch())
        );",
    )?;

    let migrations: &[(&str, &str)] = &[
        ("0000_lethal_scalphunter", include_str!("../../drizzle_sql/0000_lethal_scalphunter.sql")),
        ("0001_nostalgic_lenny_balinger", include_str!("../../drizzle_sql/0001_nostalgic_lenny_balinger.sql")),
        ("0002_fearless_photon", include_str!("../../drizzle_sql/0002_fearless_photon.sql")),
        ("0003_fancy_thunderball", include_str!("../../drizzle_sql/0003_fancy_thunderball.sql")),
        ("0004_pinned_workspaces", include_str!("../../drizzle_sql/0004_pinned_workspaces.sql")),
        ("0005_window_state", include_str!("../../drizzle_sql/0005_window_state.sql")),
        ("0006_time_entries", include_str!("../../drizzle_sql/0006_time_entries.sql")),
        ("0007_chat_session_names", include_str!("../../drizzle_sql/0007_chat_session_names.sql")),
        ("0008_chat_pinned", include_str!("../../drizzle_sql/0008_chat_pinned.sql")),
        ("0009_workspace_sessions", include_str!("../../drizzle_sql/0009_workspace_sessions.sql")),
        ("0010_active_workspaces", include_str!("../../drizzle_sql/0010_active_workspaces.sql")),
        ("0011_add_indexes", include_str!("../../drizzle_sql/0011_add_indexes.sql")),
        ("0012_agent_mode", include_str!("../../drizzle_sql/0012_agent_mode.sql")),
        ("0013_agent_mode_selector", include_str!("../../drizzle_sql/0013_agent_mode_selector.sql")),
        ("0014_agent_sessions", include_str!("../../drizzle_sql/0014_agent_sessions.sql")),
        ("0015_workspace_tiers", include_str!("../../drizzle_sql/0015_workspace_tiers.sql")),
        ("0016_rename_tiers_to_states", include_str!("../../drizzle_sql/0016_rename_tiers_to_states.sql")),
        ("0017_fix_maintenance_state", include_str!("../../drizzle_sql/0017_fix_maintenance_state.sql")),
        ("0018_rename_pod_to_coordinator", include_str!("../../drizzle_sql/0018_rename_pod_to_coordinator.sql")),
        ("0019_workspace_nav_visible", include_str!("../../drizzle_sql/0019_workspace_nav_visible.sql")),
        ("0020_heartbeat_schedule", include_str!("../../drizzle_sql/0020_heartbeat_schedule.sql")),
        ("0021_rename_coordinator_to_manager", include_str!("../../drizzle_sql/0021_rename_coordinator_to_manager.sql")),
        ("0022_agent_sessions_table", include_str!("../../drizzle_sql/0022_agent_sessions_table.sql")),
        ("0023_workspace_relations", include_str!("../../drizzle_sql/0023_workspace_relations.sql")),
        ("0024_activity_feed", include_str!("../../drizzle_sql/0024_activity_feed.sql")),
        ("0025_activity_feed_read", include_str!("../../drizzle_sql/0025_activity_feed_read.sql")),
        ("0026_heartbeat_fires", include_str!("../../drizzle_sql/0026_heartbeat_fires.sql")),
        ("0027_wakes_since_compact", include_str!("../../drizzle_sql/0027_wakes_since_compact.sql")),
        ("0028_agent_heartbeats", include_str!("../../drizzle_sql/0028_agent_heartbeats.sql")),
        ("0029_heartbeat_fires_schedule_name", include_str!("../../drizzle_sql/0029_heartbeat_fires_schedule_name.sql")),
        ("0030_code_migrations", include_str!("../../drizzle_sql/0030_code_migrations.sql")),
        ("0031_skill_regen_version", include_str!("../../drizzle_sql/0031_skill_regen_version.sql")),
        ("0032_add_use_session_stream", include_str!("../../drizzle_sql/0032_add_use_session_stream.sql")),
        ("0033_agent_session_terminal_id_namespace", include_str!("../../drizzle_sql/0033_agent_session_terminal_id_namespace.sql")),
        ("0034_heartbeat_session_archive_show", include_str!("../../drizzle_sql/0034_heartbeat_session_archive_show.sql")),
        ("0035_heartbeat_concurrency_policy", include_str!("../../drizzle_sql/0035_heartbeat_concurrency_policy.sql")),
        ("0036_heartbeat_active_session", include_str!("../../drizzle_sql/0036_heartbeat_active_session.sql")),
        ("0037_agent_session_active_terminal", include_str!("../../drizzle_sql/0037_agent_session_active_terminal.sql")),
        ("0038_rename_workspace_sessions_to_layouts", include_str!("../../drizzle_sql/0038_rename_workspace_sessions_to_layouts.sql")),
        ("0039_agent_sessions_to_workspace_sessions", include_str!("../../drizzle_sql/0039_agent_sessions_to_workspace_sessions.sql")),
        ("0040_rename_agent_heartbeats", include_str!("../../drizzle_sql/0040_rename_agent_heartbeats.sql")),
        ("0041_activity_feed_workspace_keyed", include_str!("../../drizzle_sql/0041_activity_feed_workspace_keyed.sql")),
        ("0042_canonical_key_drop_agent_suffix", include_str!("../../drizzle_sql/0042_canonical_key_drop_agent_suffix.sql")),
        ("0043_heartbeat_use_workspace_session", include_str!("../../drizzle_sql/0043_heartbeat_use_workspace_session.sql")),
        ("0044_clear_ghost_heartbeat_active_terminals", include_str!("../../drizzle_sql/0044_clear_ghost_heartbeat_active_terminals.sql")),
        ("0045_workspace_tab_sessions", include_str!("../../drizzle_sql/0045_workspace_tab_sessions.sql")),
        ("0046_prune_legacy_layout_tables", include_str!("../../drizzle_sql/0046_prune_legacy_layout_tables.sql")),
        ("0047_drop_agent_sessions_archive", include_str!("../../drizzle_sql/0047_drop_agent_sessions_archive.sql")),
        ("0048_prune_index_artifacts", include_str!("../../drizzle_sql/0048_prune_index_artifacts.sql")),
        ("0049_drop_lead_sentinel_in_activity_feed", include_str!("../../drizzle_sql/0049_drop_lead_sentinel_in_activity_feed.sql")),
        // 0050 (added in 0.39.0): app_settings table — superseded by ~/.k2so/settings.json in same release; kept for rollback safety, not read by current code.
        ("0050_app_settings", include_str!("../../drizzle_sql/0050_app_settings.sql")),
        // 0051 (added in 0.39.0): dedupe symmetric workspace_relations rows. Phase 2.5b
        // workspace==agent insight — a connection between two workspaces implies bidirectional
        // awareness, so explicit A→B + B→A pairs collapse to one row with merged relation_type.
        ("0051_dedup_symmetric_workspace_relations", include_str!("../../drizzle_sql/0051_dedup_symmetric_workspace_relations.sql")),
        // 0052 (added in 0.39.39): #676 daemon-canonical tab_titles table +
        // #677.3 workspace_layouts.revision (monotonic LWW tab-order).
        ("0052_tab_titles_and_layout_revision", include_str!("../../drizzle_sql/0052_tab_titles_and_layout_revision.sql")),
        // 0053 (added in 0.39.46): #676 follow-up — a `locked` flag on
        // tab_titles so a user's explicit rename is STICKY and never
        // overwritten by a program-generated PTY title.
        ("0053_tab_title_locked", include_str!("../../drizzle_sql/0053_tab_title_locked.sql")),
        // 0054 (#67): per-workspace remote-instruct opt-in column on
        // `projects` (default 0/OFF, fail-closed). Refines the Composer 1c
        // connect-user gate from an app-level flag to a per-workspace
        // opt-in; the app-level flag stays a global master (back-compat).
        ("0054_project_allow_remote_instruct", include_str!("../../drizzle_sql/0054_project_allow_remote_instruct.sql")),
        // 0055 (GAP #3): cross-server connections. `workspace_remote_connections`
        // links a LOCAL source workspace to a remote `<agent>@<host>`; it is the
        // gate for agent-initiated cross-daemon sends (federation::handle_send
        // fails closed unless the source workspace has a row for the target).
        ("0055_workspace_remote_connections", include_str!("../../drizzle_sql/0055_workspace_remote_connections.sql")),
        // 0056 (B3a sandbox): per-workspace Anthropic API key (BYO key) on
        // `projects` (nullable TEXT, no default). Staged as ANTHROPIC_API_KEY
        // into a microVM-backed cell's guest env at spawn so the in-cell
        // Claude Code skips interactive auth. PLAINTEXT at rest (root-only box
        // DB); at-rest encryption is a follow-up. Never logged.
        ("0056_project_anthropic_api_key", include_str!("../../drizzle_sql/0056_project_anthropic_api_key.sql")),
        // 0058 (P3a sandbox / K2-as-a-server): the `api_keys` table — the
        // first-class, owner-minted, revocable API-key auth tier for the
        // external `/v1/*` surface. Stores SHA-256(raw key) (NOT argon2 — the
        // key is high-entropy CSPRNG, no dictionary to grind) + an optional BYO
        // anthropic_api_key staged into the sessions this key spawns. scope
        // column reserved for per-tenant (P4); revoked_at gives immediate,
        // durable revocation. Raw key + anthropic key are never logged.
        // (0057 is reserved for the P1/P2a sandbox-seam line; P3a slots in at
        // 0058 so it rebases cleanly onto that work.)
        ("0058_api_keys", include_str!("../../drizzle_sql/0058_api_keys.sql")),
        // 0059 (sandbox v2 / workspace-scoped sessions, PRD §G2 #4): the
        // per-key WORKSPACE GRANT column `allowed_workspaces` on `api_keys`.
        // The tenancy seam — a key can be scoped to an explicit set of
        // workspace slugs (or `"*"` for all). FAIL-CLOSED: NULL (the value
        // existing rows backfill to) = NO grant, so a key never silently
        // reaches a workspace it was never scoped to. Owner-token principals
        // bypass the grant (owner = all). Slugs are non-secret (the list route
        // may surface them); unlike the anthropic key, nothing here is hidden.
        ("0059_api_key_workspace_grant", include_str!("../../drizzle_sql/0059_api_key_workspace_grant.sql")),
        // 0060 (sandbox v2 / workspace-scoped sessions, PRD §G2 #1): per-workspace
        // FS MODE column `sandbox_fs_mode` on `projects` (nullable TEXT).
        // 'overlay' (default) vs 'ro+scratch'. NULL/absent/unknown → 'overlay'
        // at read time (`FsMode::from_setting`) — fail-safe to the RO-base
        // default; the write path validates the exact enum before storing.
        ("0060_project_sandbox_fs_mode", include_str!("../../drizzle_sql/0060_project_sandbox_fs_mode.sql")),
        // 0061 (sandbox v2 / fs-mirror PRD §5): the host BRIDGE index
        // `sandbox_sessions` — one row per workspace-scoped MIRROR sandbox
        // session (session_id → workspace, sandbox home, `.jsonl` real path,
        // `/work` layer). Powers the per-workspace audit LIST + the resume
        // re-mount lookup. Separate from the canonical `workspace_sessions`.
        ("0061_sandbox_sessions_index", include_str!("../../drizzle_sql/0061_sandbox_sessions_index.sql")),
        // 0062 (heartbeat reliability overhaul): failure backoff columns
        // (`consecutive_failures`, `next_retry_at`, `disabled_reason`),
        // the visible `schedule_error` state for unparseable specs, and
        // the `scheduler_meta` KV (first key: `last_tick_at`) that makes
        // tick-transport gaps measurable. See
        // `.k2/notes/heartbeat-misfire-study.md`.
        ("0062_heartbeat_reliability", include_str!("../../drizzle_sql/0062_heartbeat_reliability.sql")),
        // 0063 (agent de-generalization S1): per-workspace DEFAULT AGENT
        // column `default_agent` on `projects` (nullable TEXT, no default).
        // Holds an `agent_presets` preset id (UUID) — readers also tolerate
        // a legacy command token like "claude". NULL = inherit the global
        // `AppSettings.default_agent` at resolve time; existing rows
        // backfill to NULL (non-retroactive), new rows are stamped with the
        // current global default at creation.
        ("0063_project_default_agent", include_str!("../../drizzle_sql/0063_project_default_agent.sql")),
        // 0064 (feedback F1, prd-agent-feedback-notifications §4.1):
        // `feedback` (durable agent→human asks: kind/title/body/options/
        // priority/status/answer + session deep-link fields) +
        // `feedback_comments` (per-item discussion thread, FK CASCADE) +
        // the (project_id, status, created_at) list index. Additive.
        ("0064_feedback", include_str!("../../drizzle_sql/0064_feedback.sql")),
        // 0065 (presence/multiplayer S7a, prd-presence-multiplayer-v1
        // §5.5): pin-to-size persistence — `pinned_cols`/`pinned_rows`/
        // `pinned_set_by` on `workspace_tab_sessions` (all NULL =
        // unpinned; existing rows backfill to NULL). Read back at
        // session registration so a pin survives daemon restart.
        ("0065_pinned_size", include_str!("../../drizzle_sql/0065_pinned_size.sql")),
        // 0066 (projects V1 P1, prd-projects-v1 §3): `project_groups`
        // (named groups of workspaces; poc_workspace_id is a PLAIN column,
        // no FK — enforcement is route-level per resolved Q6) +
        // `project_group_members` (many-to-many, UNIQUE pair, CASCADE) +
        // `project_group_messages` (ONE chat stream per group) +
        // `project_group_dashboards` (canonical shared layouts; V1
        // auto-creates 'Main'). NOT the legacy `projects`/`workspaces`
        // tables — those stay untouched. Additive.
        ("0066_project_groups", include_str!("../../drizzle_sql/0066_project_groups.sql")),
        // 0067 (projects V1 §6.7.7): project-group `icon` (dataUrl TEXT,
        // NULL = unset — a group has no folder to detect from, so the
        // icon lives IN the row and is served by a dedicated route, not
        // the list/show payloads) + `color` (`#rrggbb` TEXT, NULL = the
        // renderer derives a stable fallback). Additive.
        ("0067_project_group_icon", include_str!("../../drizzle_sql/0067_project_group_icon.sql")),
        // 0068 (companion C4, prd-companion-v2 §4; feedback PRD §8.4
        // Option B): `push_devices` — the daemon-held mobile push
        // registry (device_id PK, platform apns|fcm, vendor token,
        // daemon-resolved username, last_seen_at). Companion upserts
        // via `/cli/push/register-device` on every app launch; the
        // relay gateway stays stateless. Token-indexed for the
        // dead-token (410 Unregistered) prune path. Additive.
        ("0068_push_devices", include_str!("../../drizzle_sql/0068_push_devices.sql")),
        // 0069 (host sessions F1, prd-v1-api-completion §3): per-workspace
        // `projects.api_skip_permissions` — whether API-spawned HOST sessions
        // keep the agent preset's dangerous auto-approve flags. NULL at add
        // time; product default flipped to ON in 0093 (headless /v1). See
        // get_api_skip_permissions (NULL → true; explicit 0 = owner opt-out).
        (
            "0069_project_api_skip_permissions",
            include_str!("../../drizzle_sql/0069_project_api_skip_permissions.sql"),
        ),
        // 0070 (W2, 0.40.30): agent-preset METADATA — `danger_flags`
        // (JSON array: the preset's OWN dangerous auto-approve flags,
        // unioned with the host-session policy's hardcoded floor so a
        // custom agent's flag fails CLOSED on API spawn), `env` (JSON
        // object merged into the child env at spawn, under AGENT.md /
        // K2-internal env), `readiness` ('bracketed-paste' |
        // 'settle:<ms>', the InjectionProfile vocabulary). All NULLable;
        // NULL = legacy/unknown (consumers fail closed). Built-in seeds
        // are backfilled label-keyed by `seed_agent_presets`. Additive.
        (
            "0070_agent_preset_metadata",
            include_str!("../../drizzle_sql/0070_agent_preset_metadata.sql"),
        ),
        // 0071 (W5, 0.40.30): API-key LLM-credential PROVIDER metadata —
        // `api_keys.provider` (canonical 'anthropic'|'openai'|'google'|'xai';
        // NULL = anthropic, the pre-0071 behavior, so existing rows stage
        // ANTHROPIC_API_KEY byte-identically; unknown values fail CLOSED at
        // staging: nothing staged) + `api_keys.base_url` (optional endpoint
        // override, staged as OPENAI_BASE_URL for provider 'openai' only).
        // Additive.
        (
            "0071_api_key_provider",
            include_str!("../../drizzle_sql/0071_api_key_provider.sql"),
        ),
        // 0072 (GH#22/#23/#24): heal junk per-project heartbeat schedules
        // written by pre-0.40.41 CLIs that misparsed `--help`/subcommand
        // words as a schedule frequency and POSTed them to the legacy
        // /cli/heartbeat/schedule route verbatim. Resets rows with an
        // invalid heartbeat_mode, or mode='scheduled' whose schedule JSON
        // is missing/malformed or carries a $.frequency outside
        // daily/weekly/monthly/yearly, to off + NULL + disabled.
        // json_valid() guards json_extract() so malformed JSON can't
        // abort the migration. The route now rejects these writes
        // (misc_routes.rs validate_heartbeat_schedule_write).
        (
            "0072_clear_junk_heartbeat_schedules",
            include_str!("../../drizzle_sql/0072_clear_junk_heartbeat_schedules.sql"),
        ),
        // 0073: user-selectable heartbeat delivery session —
        // `workspace_heartbeats.session_provider` records which
        // provider's session store `last_session_id` belongs to when
        // the user pins a heartbeat to a SPECIFIC saved session. NULL
        // (the backfill) = the workspace default agent, the pre-0073
        // behavior. Additive.
        (
            "0073_heartbeat_session_provider",
            include_str!("../../drizzle_sql/0073_heartbeat_session_provider.sql"),
        ),
        // 0074: workspace-attributed nested subdomains — daemon-local
        // `label → project_id` attribution overlay for the K2 Connect
        // nested-subdomain routing map. Written by the create/point/
        // claim seams, removed by rm/unclaim; the routing map itself
        // stays control-plane-owned. Additive.
        (
            "0074_subdomain_workspaces",
            include_str!("../../drizzle_sql/0074_subdomain_workspaces.sql"),
        ),
        // 0075 (K2 Mail foundation, prd-email-server-v1 §12): the
        // K2-side mail state — `mail_server` (singleton install
        // record; not-installed = no row) + `mail_domains` (normalized
        // punycode domains + per-record DNS verification state) +
        // `mail_relay_configs` (smart-host creds, kind anticipates V2
        // providers) + `mail_addresses` (agent↔address ownership +
        // idempotent-minting client_id) + `mail_outbound` (approval
        // queue AND send audit log — a row lands BEFORE any hand-off
        // to Stalwart, fail-closed) + `mail_doctor_runs` (deliverability
        // history) + per-workspace `projects.mail_agent_send` /
        // `projects.mail_address_cap` gating overrides (NULL = inherit
        // the global AppSettings defaults: 'off' / 5). Additive.
        ("0075_mail", include_str!("../../drizzle_sql/0075_mail.sql")),
        // 0073 (K2 Mail S1, prd-email-server-v1 §4.1/§5.2): the enable
        // flow's resumable-state-machine columns on `mail_server` —
        // `enable_progress_json` (per-step completion, polled by
        // GET /cli/mail/status; re-enable resumes) + `last_error`
        // (most recent supervisor failure, surfaced verbatim).
        // Additive.
        (
            "0076_mail_enable_progress",
            include_str!("../../drizzle_sql/0076_mail_enable_progress.sql"),
        ),
        // 0074 (K2 Mail S9, prd-email-server-v1 §17.5):
        // `mail_external_inboxes` — the user's OWN external accounts
        // (IMAP in V1), each bound to exactly ONE workspace whose
        // agents read the inbox + save reply DRAFTS into the account's
        // real Drafts folder (no send path exists). Credentials live
        // in the daemon vault under `ext-inbox-<id>`, never in a
        // column. Additive.
        (
            "0077_mail_external_inboxes",
            include_str!("../../drizzle_sql/0077_mail_external_inboxes.sql"),
        ),
        // 0078 (K2 Mail S10, prd-email-server-v1 §17.5): per-role
        // access grants on external inboxes — the owner workspace keeps
        // full read+draft + sole management, additional workspaces get
        // a 'read' or 'draft' grant row. `external remove` cascades
        // these in code (inbox_id is not a FK — the 0064 idiom).
        // Additive.
        (
            "0078_mail_external_inbox_grants",
            include_str!("../../drizzle_sql/0078_mail_external_inbox_grants.sql"),
        ),
        // 0079 (K2 Mail S11, prd-email-server-v1 §17.5): ONE access
        // layer over BOTH provisioning sources. `owner_project_id`
        // keeps its name but is now the PRIMARY (managing) workspace;
        // a new `primary_level` column (hosted 'send' / linked 'draft')
        // records the primary's own ceiling. `mail_inbox_grants`
        // (source + inbox_id + project_id → level) replaces S10's
        // linked-only `mail_external_inbox_grants`: existing rows are
        // data-migrated in as source='linked', then the old table is
        // dropped. read < draft < send; 'send' is hosted-only. Runs
        // once by name (the ADD COLUMNs never double-apply).
        (
            "0079_mail_unified_grants",
            include_str!("../../drizzle_sql/0079_mail_unified_grants.sql"),
        ),
        // 0080 (K2 Mail — linked send, prd-email-server-v1 §17.5): the
        // OPT-IN SMTP send path for LINKED external inboxes. Nullable
        // `smtp_host` / `smtp_port` / `smtp_tls` override columns on
        // `mail_external_inboxes` — NULL = derive the submission server
        // from the provider / IMAP host at send time; a value pins it.
        // Auth reuses the vaulted `ext-inbox-<id>` app-password. Additive.
        (
            "0080_mail_external_smtp",
            include_str!("../../drizzle_sql/0080_mail_external_smtp.sql"),
        ),
        // 0081 (K2 Mail — inbox management + delete, prd-email-server-v1
        // §17.5): two per-workspace booleans ORTHOGONAL to `level` —
        // `can_manage` (move/folders/flags/archive) + `can_delete`
        // (delete = MOVE to Trash, never EXPUNGE; requires can_manage).
        // On `mail_inbox_grants` for grantees, and `primary_can_manage`
        // / `primary_can_delete` on BOTH `mail_addresses` and
        // `mail_external_inboxes` for the primary's own caps. Default
        // OFF everywhere (opt-in). Additive ADD COLUMNs, run once by name.
        (
            "0081_mail_manage_caps",
            include_str!("../../drizzle_sql/0081_mail_manage_caps.sql"),
        ),
        // 0082 (K2 Mail — OAuth linked inboxes, prd-email-oauth-providers-v1
        // slice O1): three additive columns on `mail_external_inboxes` —
        // `auth_kind` ('password'|'oauth', default 'password'),
        // `provider` ('gmail'|'microsoft'|NULL), `token_expires_at` (unix
        // secs; the only non-secret token bit — tokens themselves vault
        // under `ext-inbox-<id>-oauth`). ALSO widens the `kind` CHECK to
        // admit 'graph' (Phase-2 Graph backend). SQLite can't ALTER a
        // CHECK, so it's a safe TABLE REBUILD (create-copy-drop-rename +
        // recreate index) inside the one migration transaction; no FK
        // references this table so the drop is safe. Runs once by name.
        (
            "0082_mail_oauth",
            include_str!("../../drizzle_sql/0082_mail_oauth.sql"),
        ),
        // 0083 (DNS K1): per-workspace DNS-manage opt-in column on
        // `projects` (default 0/OFF, fail-closed). App-level
        // `dnsManageEnabled` stays a global master (OR'd on top).
        (
            "0083_project_dns_manage_enabled",
            include_str!("../../drizzle_sql/0083_project_dns_manage_enabled.sql"),
        ),
        (
            "0084_mail_outbound_scheduled",
            include_str!("../../drizzle_sql/0084_mail_outbound_scheduled.sql"),
        ),
        // 0085 (C1): per-workspace agents-may-create-connections opt-in
        // on `projects` (default 0/OFF, fail-closed). App-level
        // `agentsCanCreateConnections` stays a global master (OR'd on top).
        // Owner / Owner-role always bypass; agents need the effective OR.
        (
            "0085_project_agents_can_create_connections",
            include_str!("../../drizzle_sql/0085_project_agents_can_create_connections.sql"),
        ),
        // 0086 (Phase 0, prd-wiki-public-chat-api-loopback-v1): per-key
        // capability flags on `api_keys` — host_sessions / canonical_message
        // / sandboxes. DEFAULT 1 backfills EXISTING rows to all-doors-on
        // (today's behavior); new mints write host-only via create_api_key.
        // Additive.
        (
            "0086_api_key_capabilities",
            include_str!("../../drizzle_sql/0086_api_key_capabilities.sql"),
        ),
        // 0087 (Phase 0b, prd-wiki-public-chat-api-loopback-v1): per-workspace
        // owner API guest policy text. NULL/empty → platform default; daemon
        // injects on every host-session spawn + message-live. Additive.
        (
            "0087_project_api_guest_policy",
            include_str!("../../drizzle_sql/0087_project_api_guest_policy.sql"),
        ),
        // 0088 (Phase 1, prd-wiki-public-chat-api-loopback-v1): per-workspace
        // public wiki chat opt-in. DEFAULT 0 (OFF) — serve alone never enables
        // chat (D6). Additive.
        (
            "0088_project_wiki_public_chat",
            include_str!("../../drizzle_sql/0088_project_wiki_public_chat.sql"),
        ),
        // 0089 — Remote Session Layer 0: grants shell + events audit log
        // (denials always recorded). Master switch is app_settings
        // remote_sessions_enabled (default OFF), independent of grants.
        (
            "0089_remote_sessions",
            include_str!("../../drizzle_sql/0089_remote_sessions.sql"),
        ),
        // 0090 (Tickets UX): status `planned` + multi-assignee usernames
        // snapshotted for push targeting (survives connect-user removal).
        // Wire table stays `feedback*` for CLI/API compat; UI says Tickets.
        (
            "0090_feedback_tickets",
            include_str!("../../drizzle_sql/0090_feedback_tickets.sql"),
        ),
        // 0091 (Context context management stack): optional AGENTS.md layers as path
        // references per project (order + enabled + source + label).
        // Bodies never stored; empty stack = today's compose.
        (
            "0091_project_context_layers",
            include_str!("../../drizzle_sql/0091_project_context_layers.sql"),
        ),
        // 0092 (Context context management stack): system layers (AGENT / PROJECT / Tooling)
        // are toggleable, default ON.
        (
            "0092_project_context_system_flags",
            include_str!("../../drizzle_sql/0092_project_context_system_flags.sql"),
        ),
        // 0093 (prd-api-skip-permissions-default-on-v1): /v1 host-sessions are
        // headless — default api_skip_permissions ON (NULL → 1 backfill).
        // Explicit 0 remains owner opt-out. See get_api_skip_permissions.
        (
            "0093_api_skip_permissions_default_on",
            include_str!("../../drizzle_sql/0093_api_skip_permissions_default_on.sql"),
        ),
        // 0094 (Settings API keys UI): soft-disable for emergency off/on
        // without reminting the secret. resolve_api_key rejects when set.
        (
            "0094_api_key_disabled_at",
            include_str!("../../drizzle_sql/0094_api_key_disabled_at.sql"),
        ),
        // 0095 — per-workspace concurrent host-session cell cap.
        // NULL = inherit daemon default (env K2_SANDBOX_WORKSPACE_CELL_CAP
        // or 15). See get_host_session_cell_cap + CLI host-session-cell-cap.
        (
            "0095_project_host_session_cell_cap",
            include_str!("../../drizzle_sql/0095_project_host_session_cell_cap.sql"),
        ),
        // 0096 — durable host-session spawn queue (FIFO per workspace).
        // Feature default OFF (K2_HOST_SESSION_SPAWN_QUEUE). Prompt purged
        // on terminal states; capability JWTs never stored.
        (
            "0096_host_session_spawn_queue",
            include_str!("../../drizzle_sql/0096_host_session_spawn_queue.sql"),
        ),
        // 0097 — user chat session archive/restore (Claude physical MOVE).
        // Soft-archive when source file missing; dual-query list keeps all
        // archived rows visible beyond the top-100 active cap.
        (
            "0097_chat_session_archived",
            include_str!("../../drizzle_sql/0097_chat_session_archived.sql"),
        ),
        // 0098 — tickets status `needs_discussion` (orange board state;
        // waiting stays open but uses yellow in the UI).
        (
            "0098_feedback_needs_discussion",
            include_str!("../../drizzle_sql/0098_feedback_needs_discussion.sql"),
        ),
        // 0099 — durable from_api on workspace_tab_sessions + per-workspace
        // hide_api_sessions (Chat history API section / close-as-minimize).
        (
            "0099_api_session_origin_and_hide",
            include_str!("../../drizzle_sql/0099_api_session_origin_and_hide.sql"),
        ),
        // 0100 — per-workspace compose-bar send history (Up/Down).
        (
            "0100_workspace_compose_send_history",
            include_str!("../../drizzle_sql/0100_workspace_compose_send_history.sql"),
        ),
        // 0101 — per-workspace completion chime mute (default ON).
        (
            "0101_workspace_completion_sound",
            include_str!("../../drizzle_sql/0101_workspace_completion_sound.sql"),
        ),
        // 0102 — durable sidecar ordinals (workspace/handle addressing).
        (
            "0102_workspace_session_handles",
            include_str!("../../drizzle_sql/0102_workspace_session_handles.sql"),
        ),
        // 0103 — workspace Agent Name vs Handle split.
        (
            "0103_workspace_agent_handle",
            include_str!("../../drizzle_sql/0103_workspace_agent_handle.sql"),
        ),
        // 0104 — daemon-owned published services (process + desired state).
        (
            "0104_published_services",
            include_str!("../../drizzle_sql/0104_published_services.sql"),
        ),
        // 0105 — daemon-owned workspace resources (explicit file list).
        (
            "0105_workspace_resources",
            include_str!("../../drizzle_sql/0105_workspace_resources.sql"),
        ),
        // 0106 — per-workspace default model + force-on-resume + spawn-queue model.
        (
            "0106_workspace_default_model",
            include_str!("../../drizzle_sql/0106_workspace_default_model.sql"),
        ),
        // 0107 — poison sandbox stamp on the pinned conversation id → canonical.
        (
            "0107_feedback_canonical_kind_repair",
            include_str!("../../drizzle_sql/0107_feedback_canonical_kind_repair.sql"),
        ),
        // 0108 — workspace data sidecar catalog (Postgres supervisor).
        // Singleton `sql_server` + `sql_databases` + per-workspace
        // `db_agent_access` / `db_active_cap` + fail-closed `api_keys.cap_db`.
        (
            "0108_sql",
            include_str!("../../drizzle_sql/0108_sql.sql"),
        ),
        // 0109 — sql_grants (read|write + can_manage) + sql_databases.bind_role.
        (
            "0109_sql_grants",
            include_str!("../../drizzle_sql/0109_sql_grants.sql"),
        ),
        // 0110 — overlay threads catalog (named conversation_id, not v2_session_map).
        (
            "0110_overlay_conversations",
            include_str!("../../drizzle_sql/0110_overlay_conversations.sql"),
        ),
        // 0111 — per-LLM inject keystroke flow on agent_presets (NULL = default).
        (
            "0111_preset_inject_flow",
            include_str!("../../drizzle_sql/0111_preset_inject_flow.sql"),
        ),
        // 0112 — Skin Hydra sidecar singleton (enabled on/off). Enable skins
        // does not start Hydra. Absence of the row is off.
        (
            "0112_skin_hydra",
            include_str!("../../drizzle_sql/0112_skin_hydra.sql"),
        ),
    ];

    for (name, sql) in migrations {
        let already_applied: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;

        if !already_applied {
            // Run each migration inside a transaction for atomicity.
            // Retry the entire transaction on lock contention.
            let mut last_err = None;
            for attempt in 0..5u32 {
                match run_single_migration(conn, name, sql) {
                    Ok(_) => { last_err = None; break; },
                    Err(e) => {
                        let msg = e.to_string();
                        if (msg.contains("database is locked") || msg.contains("schema is locked"))
                            && attempt < 4
                        {
                            log_debug!("[db] Migration {}: locked, retrying ({}/5)", name, attempt + 1);
                            std::thread::sleep(std::time::Duration::from_millis(50 * (attempt as u64 + 1)));
                            last_err = Some(e);
                            continue;
                        }
                        return Err(e);
                    }
                }
            }
            if let Some(e) = last_err {
                return Err(e);
            }
        }
    }

    Ok(())
}

/// Execute a single migration file's statements inside a transaction.
/// "already exists" / "duplicate column" errors are silently skipped (idempotent).
fn run_single_migration(conn: &Connection, name: &str, sql: &str) -> Result<()> {
    conn.execute_batch("BEGIN;")?;
    for statement in sql.split("--> statement-breakpoint") {
        let trimmed = statement.trim();
        if !trimmed.is_empty() {
            if let Err(e) = conn.execute_batch(trimmed) {
                let msg = e.to_string();
                if msg.contains("already exists") || msg.contains("duplicate column") {
                    log_debug!("[db] Migration {}: skipping ({})", name, msg);
                    continue;
                }
                // Rollback on real errors
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(e);
            }
        }
    }
    conn.execute(
        "INSERT INTO _migrations (name) VALUES (?1)",
        params![name],
    )?;
    conn.execute_batch("COMMIT;")?;
    Ok(())
}

/// Check whether a code-side migration with the given id has been recorded.
///
/// "Code migrations" are one-time runtime passes (filesystem rewrites,
/// legacy-type coercion, etc.) whose only job is to get from state A to
/// state B. Gating them behind this check turns every launch after the
/// first into a no-op for that pass, instead of re-scanning the whole
/// workspace tree to confirm there's nothing to do.
///
/// The table (`code_migrations`, added in migration 0030) is created
/// lazily at startup; callers before migration 0030 has run see `false`
/// and safely fall through to running the migration.
pub fn has_code_migration_applied(conn: &Connection, id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM code_migrations WHERE id = ?1 LIMIT 1",
        params![id],
        |_| Ok(()),
    )
    .is_ok()
}

/// Record that a code migration completed successfully. Idempotent
/// (INSERT OR IGNORE) so repeat callers during a partial-completion
/// scenario don't error. Takes a free-form `notes` string for future
/// debugging — store counts, version numbers, anything small.
pub fn mark_code_migration_applied(conn: &Connection, id: &str, notes: Option<&str>) {
    let _ = conn.execute(
        "INSERT OR IGNORE INTO code_migrations (id, applied_at, notes) \
         VALUES (?1, unixepoch(), ?2)",
        params![id, notes],
    );
}

/// Seed built-in agent presets.
///
/// Existing users may have re-ordered or customized their presets — to
/// avoid clobbering that, we INSERT new entries by *label* uniqueness
/// (not id), and never UPDATE existing rows. The id column on built-ins
/// is otherwise ignored once a row exists, since older versions of
/// `db/mod.rs` and `commands/agents.rs` disagreed on which id mapped
/// to which label for Pi/Goose/Ollama/Interpreter.
pub(crate) fn seed_agent_presets(conn: &Connection) -> Result<()> {
    // Migration: drop Code Puppy from existing DBs (removed as a built-in
    // in this version — users can still add it as a custom preset).
    conn.execute(
        "DELETE FROM agent_presets WHERE id = ?1 AND is_built_in = 1",
        params!["b0a1c2d3-e4f5-6789-abcd-ef0123456008"],
    )?;

    // Default order for fresh installs. Existing built-ins keep their
    // current sort_order — `INSERT … WHERE NOT EXISTS` only inserts
    // entries the user is missing entirely (e.g. Pi on upgrade).
    let presets: &[(&str, &str, &str, &str, i64)] = &[
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456001", "Claude", "claude --dangerously-skip-permissions", "", 0),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456002", "Codex", "codex --yolo", "", 1),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456014", "Grok", "grok --always-approve", "", 2),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456003", "Gemini", "gemini --yolo", "", 3),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456006", "Cursor Agent", "cursor-agent", "", 4),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456012", "Pi", "pi", "", 5),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456013", "Hermes", "hermes", "", 6),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456007", "OpenCode", "opencode", "", 7),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456011", "Goose", "goose", "", 8),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456005", "Aider", "aider", "", 9),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456009", "Ollama", "ollama run llama3.2", "", 10),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456004", "Copilot", "copilot --allow-all", "", 11),
        ("b0a1c2d3-e4f5-6789-abcd-ef0123456010", "Interpreter", "interpreter", "", 12),
    ];

    for (id, label, command, icon, sort_order) in presets {
        conn.execute(
            "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             SELECT ?1, ?2, ?3, ?4, 1, ?5, 1 \
             WHERE NOT EXISTS (SELECT 1 FROM agent_presets WHERE label = ?2 AND is_built_in = 1)",
            params![id, label, command, icon, sort_order],
        )?;
    }

    // One-time: the old Codex default was a long `-c model_reasoning_effort`
    // + `--dangerously-bypass-approvals-and-sandbox` line. Replace that
    // exact stock command with `codex --yolo`. Do not touch a row the
    // user already customized to something else.
    conn.execute(
        "UPDATE agent_presets SET command = 'codex --yolo' \
         WHERE is_built_in = 1 AND label = 'Codex' \
           AND command = 'codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox'",
        [],
    )?;

    // Migration-0070 metadata backfill — fresh installs AND upgrades. The
    // INSERT above deliberately leaves the metadata columns NULL (and
    // existing installed rows were backfilled to NULL by the ALTERs), so
    // this single label-keyed COALESCE pass is what stamps truthful
    // metadata on the built-ins, idempotently, without ever clobbering a
    // non-NULL value.
    backfill_built_in_preset_metadata(conn)?;

    // One-time ordering repair: on upgrade the `WHERE NOT EXISTS` insert placed
    // Hermes at sort_order 5, but existing installs still had OpenCode at 5 — a
    // tie that renders Hermes AFTER OpenCode. If Hermes and OpenCode still share
    // a slot, open a gap by bumping OpenCode (and everything at/after Hermes's
    // slot, except Hermes) down by one, so Hermes lands between Pi (4) and
    // OpenCode. Idempotent: once the collision is resolved this is a no-op, and
    // it never fires on fresh installs (the seed already spaces them 4/5/6).
    let hermes_opencode_collision: i64 = conn.query_row(
        "SELECT COUNT(*) FROM agent_presets h, agent_presets o \
         WHERE h.label = 'Hermes' AND o.label = 'OpenCode' \
           AND h.is_built_in = 1 AND o.is_built_in = 1 \
           AND h.sort_order = o.sort_order",
        [],
        |r| r.get(0),
    )?;
    if hermes_opencode_collision > 0 {
        conn.execute(
            "UPDATE agent_presets SET sort_order = sort_order + 1 \
             WHERE is_built_in = 1 AND label != 'Hermes' \
               AND sort_order >= (SELECT sort_order FROM agent_presets \
                                  WHERE label = 'Hermes' AND is_built_in = 1)",
            [],
        )?;
    }

    // One-time ordering repair: on upgrade the `WHERE NOT EXISTS` insert places
    // Grok at sort_order 3, but existing installs still had Cursor Agent at 3 —
    // a tie that renders Grok adjacent-but-unordered against Cursor Agent. If
    // Grok and Cursor Agent still share a slot, open a gap by bumping Cursor
    // Agent (and everything at/after Grok's slot, except Grok) down by one.
    // Idempotent: once resolved this is a no-op.
    let grok_cursor_collision: i64 = conn.query_row(
        "SELECT COUNT(*) FROM agent_presets g, agent_presets c \
         WHERE g.label = 'Grok' AND c.label = 'Cursor Agent' \
           AND g.is_built_in = 1 AND c.is_built_in = 1 \
           AND g.sort_order = c.sort_order",
        [],
        |r| r.get(0),
    )?;
    if grok_cursor_collision > 0 {
        conn.execute(
            "UPDATE agent_presets SET sort_order = sort_order + 1 \
             WHERE is_built_in = 1 AND label != 'Grok' \
               AND sort_order >= (SELECT sort_order FROM agent_presets \
                                  WHERE label = 'Grok' AND is_built_in = 1)",
            [],
        )?;
    }

    // One-time ordering repair: product order is Grok before Gemini. Older
    // seeds had Gemini at 2 and Grok at 3. Swap their sort_order when Gemini
    // still sorts earlier. Idempotent once Grok ≤ Gemini.
    let gemini_order: Option<i64> = conn
        .query_row(
            "SELECT sort_order FROM agent_presets \
             WHERE label = 'Gemini' AND is_built_in = 1",
            [],
            |r| r.get(0),
        )
        .ok();
    let grok_order: Option<i64> = conn
        .query_row(
            "SELECT sort_order FROM agent_presets \
             WHERE label = 'Grok' AND is_built_in = 1",
            [],
            |r| r.get(0),
        )
        .ok();
    if let (Some(g_ord), Some(x_ord)) = (gemini_order, grok_order) {
        if g_ord < x_ord {
            conn.execute(
                "UPDATE agent_presets SET sort_order = ?1 \
                 WHERE label = 'Grok' AND is_built_in = 1",
                params![g_ord],
            )?;
            conn.execute(
                "UPDATE agent_presets SET sort_order = ?1 \
                 WHERE label = 'Gemini' AND is_built_in = 1",
                params![x_ord],
            )?;
        }
    }

    Ok(())
}

/// Truthful migration-0070 metadata for the 13 built-in presets, keyed by
/// LABEL (the same uniqueness key `seed_agent_presets` reconciles on —
/// built-in ids historically disagreed between modules, labels never did).
///
/// Columns: (label, danger_flags JSON array, env JSON object, readiness).
///
/// - `danger_flags`: ONLY the five audited auto-approve flags, each on its
///   owner. Presets we have NOT audited stay NULL (= unknown), which the
///   host-session policy treats as "strip the hardcoded floor + warn" —
///   never as "safe". Keep in sync with
///   `v1_host_sessions::policy::DANGER_FLAGS` (the legacy floor).
/// - `env`: no built-in needs ambient env today — all NULL. The column is
///   for community/custom presets (and future seeds).
/// - `readiness`: the 2026-07 TUI signal studies behind
///   `provider_resume::injection_profile_for_provider`, same classes:
///   'bracketed-paste' (claude/grok/cursor-agent — the ?2004h flip is
///   trustworthy) or 'settle:<ms>' (codex/gemini 2000, pi 1500, hermes
///   7000 — ?2004h lies). The unstudied six stay NULL (= unknown).
const BUILT_IN_PRESET_METADATA: &[(&str, Option<&str>, Option<&str>, Option<&str>)] = &[
    ("Claude", Some(r#"["--dangerously-skip-permissions"]"#), None, Some("bracketed-paste")),
    (
        "Codex",
        Some(r#"["--dangerously-bypass-approvals-and-sandbox"]"#),
        None,
        Some("settle:2000"),
    ),
    ("Gemini", Some(r#"["--yolo"]"#), None, Some("settle:2000")),
    ("Grok", Some(r#"["--always-approve"]"#), None, Some("bracketed-paste")),
    ("Cursor Agent", None, None, Some("bracketed-paste")),
    ("Pi", None, None, Some("settle:1500")),
    ("Hermes", None, None, Some("settle:7000")),
    ("OpenCode", None, None, None),
    ("Goose", None, None, None),
    ("Aider", None, None, None),
    ("Ollama", None, None, None),
    ("Copilot", Some(r#"["--allow-all"]"#), None, None),
    ("Interpreter", None, None, None),
];

/// Stamp [`BUILT_IN_PRESET_METADATA`] onto the built-in rows. COALESCE
/// per column: only fills NULL (legacy/unknown), never overwrites a
/// value already present — idempotent across every boot's reseed and
/// safe on user-touched rows. Shared by `seed_agent_presets` and
/// `db_ops::presets_reset_built_ins` (which recreates rows metadata-less
/// and relies on this pass to restore truth).
pub(crate) fn backfill_built_in_preset_metadata(conn: &Connection) -> Result<()> {
    for (label, danger_flags, env, readiness) in BUILT_IN_PRESET_METADATA {
        conn.execute(
            "UPDATE agent_presets SET \
                 danger_flags = COALESCE(danger_flags, ?2), \
                 env          = COALESCE(env, ?3), \
                 readiness    = COALESCE(readiness, ?4) \
             WHERE label = ?1 AND is_built_in = 1",
            params![label, danger_flags, env, readiness],
        )?;
    }
    // Grok submit is paste + one Return (steer). Only fills NULL so a
    // user-edited flow is left alone. Reset built-ins recreates NULL rows
    // and this pass stamps the Grok default again.
    conn.execute(
        "UPDATE agent_presets SET inject_flow = ?1 \
         WHERE is_built_in = 1 AND label = 'Grok' AND inject_flow IS NULL",
        params![crate::inject_flow::GROK_INJECT_FLOW_JSON],
    )?;
    Ok(())
}

/// Seed the sentinel `projects` rows used by `awareness::egress`
/// when a signal's workspace doesn't resolve to a real project.
///
/// `activity_feed.project_id` has a hard FK on `projects.id`. Without
/// these rows, any signal from an unregistered workspace (CLI run in
/// a non-K2SO directory, signals from ad-hoc test harnesses, etc.)
/// would fail the FK check — audit silently drops, breaking the
/// "audit always fires" primitive promise locked in the Phase 3 PRD.
///
/// Two sentinels:
/// - `_orphan`  — fallback for `AgentAddress::Agent` / `Workspace`
///                signals whose workspace id isn't in `projects`.
/// - `_broadcast` — bucket for `AgentAddress::Broadcast` senders
///                (no single workspace attributable).
///
/// Both are upserted with INSERT OR IGNORE so re-running at boot
/// never duplicates. Paths/names are human-readable tags — they're
/// never dereferenced as filesystem paths, but showing them in a
/// `k2so projects` listing should be obvious.
/// Project ids reserved for internal audit-feed routing. These rows
/// exist in `projects` purely to satisfy the `activity_feed.project_id`
/// FK (see [`seed_audit_sentinels`]) and must NEVER surface in the
/// user-facing workspace list. UI-facing surfaces filter them out;
/// internal callers (heartbeat/agent scanning, dedup/import checks,
/// migrations) see every row via `Project::list`. Public (0.40.24 S3)
/// so the daemon's fleet-view route (`/cli/agent/list` behind
/// `k2 agent list`) can apply the same filter.
pub const AUDIT_SENTINEL_IDS: &[&str] = &["_orphan", "_broadcast"];

pub(crate) fn seed_audit_sentinels(conn: &Connection) -> Result<()> {
    let sentinels: &[(&str, &str, &str)] = &[
        ("_orphan", "_orphan", "Orphan audit bucket"),
        ("_broadcast", "_broadcast", "Broadcast audit bucket"),
    ];
    for (id, path, name) in sentinels {
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
            params![id, path, name],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Unit tests for the migration/bootstrap layer. Every test opens
    //! its own in-memory connection — the shared `init_for_tests()`
    //! handle is NOT used here because these tests assert on
    //! migration application order, PRAGMA state, and idempotency,
    //! which would be polluted by a process-wide handle.
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    fn fresh_memory() -> Connection {
        let conn = Connection::open(":memory:").unwrap();
        conn.busy_timeout(std::time::Duration::from_millis(5000))
            .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn
    }

    fn scratch_db_path() -> std::path::PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "k2so-db-mod-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        std::fs::create_dir_all(&base).unwrap();
        // Tests open a private path; either filename is fine for open().
        base.join("k2so.db")
    }

    #[test]
    fn resolve_home_db_path_prefers_k2_db_when_present() {
        let base = std::env::temp_dir().join(format!(
            "k2-db-resolve-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        // Neither → legacy create path (Stage A writer not flipped).
        assert_eq!(
            resolve_home_db_path(&base),
            base.join("k2so.db"),
            "neither file: still create k2so.db until Stage B"
        );
        std::fs::write(base.join("k2so.db"), b"old").unwrap();
        assert_eq!(
            resolve_home_db_path(&base),
            base.join("k2so.db"),
            "only legacy: honor k2so.db"
        );
        std::fs::write(base.join("k2.db"), b"new").unwrap();
        assert_eq!(
            resolve_home_db_path(&base),
            base.join("k2so.db"),
            "both tiny stubs: do not hide k2so.db behind a stray k2.db"
        );
        std::fs::remove_file(base.join("k2so.db")).unwrap();
        assert_eq!(
            resolve_home_db_path(&base),
            base.join("k2.db"),
            "only k2.db: use it"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    fn write_sqlite_with_n_projects(path: &std::path::Path, n: usize) {
        bootstrap_test_db_at(path).expect("bootstrap");
        let conn = Connection::open(path).expect("open");
        for i in 0..n {
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    format!("p{i}"),
                    format!("ws-{i}"),
                    format!("/tmp/ws-{i}")
                ],
            )
            .expect("insert project");
        }
    }

    #[test]
    fn resolve_home_db_path_dual_real_prefers_populated_k2so() {
        let base = std::env::temp_dir().join(format!(
            "k2-db-dual-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        write_sqlite_with_n_projects(&base.join("k2so.db"), 3);
        write_sqlite_with_n_projects(&base.join("k2.db"), 0);
        assert_eq!(
            resolve_home_db_path(&base),
            base.join("k2so.db"),
            "stub k2.db (sentinels only) must not hide k2so.db with live workspaces"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn resolve_home_db_path_dual_real_prefers_populated_k2() {
        let base = std::env::temp_dir().join(format!(
            "k2-db-dual-new-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        write_sqlite_with_n_projects(&base.join("k2so.db"), 0);
        write_sqlite_with_n_projects(&base.join("k2.db"), 4);
        assert_eq!(
            resolve_home_db_path(&base),
            base.join("k2.db"),
            "live k2.db wins over empty k2so.db"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn resolve_home_db_path_dual_real_equal_count_prefers_k2so() {
        let base = std::env::temp_dir().join(format!(
            "k2-db-dual-tie-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        write_sqlite_with_n_projects(&base.join("k2so.db"), 0);
        write_sqlite_with_n_projects(&base.join("k2.db"), 0);
        assert_eq!(
            resolve_home_db_path(&base),
            base.join("k2so.db"),
            "equal live counts: keep k2so.db (do not hide it behind a same-size stub k2.db)"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_home_db_path_stage_b_symlink_uses_k2() {
        let base = std::env::temp_dir().join(format!(
            "k2-db-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        write_sqlite_with_n_projects(&base.join("k2.db"), 2);
        std::os::unix::fs::symlink(base.join("k2.db"), base.join("k2so.db")).unwrap();
        assert_eq!(
            resolve_home_db_path(&base),
            base.join("k2.db"),
            "Stage B compat symlink k2so.db → k2.db is not a dual-real conflict"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // ── Migration runner ──────────────────────────────────────────
    #[test]
    fn migrations_create_core_tables() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        // Sanity: every table we unit-test in schema::unit_tests must
        // exist after migrations. Using sqlite_master to confirm.
        for table in [
            "projects",
            "workspace_sessions",
            "workspace_heartbeats",
            "agent_presets",
            "heartbeat_fires",
            "activity_feed",
            "workspace_relations",
            "focus_groups",
            "published_services",
        ] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "expected table '{}' to exist", table);
        }
    }

    #[test]
    fn migrations_are_idempotent_when_run_twice() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        let first: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        // Second run must be a no-op — every migration is already in
        // _migrations, so the `if !already_applied` guard short-circuits.
        run_migrations(&conn).unwrap();
        let second: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(first, second, "re-running migrations must not add rows");
    }

    #[test]
    fn migrations_registers_every_file_in_migrations_table() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        // The full list of migration names is internal; we can at
        // least assert the latest known migration is tracked. If a
        // new migration is added, this assertion stays truthful
        // because we check >= known recent + <= reasonable upper
        // bound.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
            .unwrap();
        assert!(n >= 30, "expected >=30 applied migrations, got {}", n);

        // Name ordering: the last applied migration's name is the
        // highest-numbered one shipped. If this breaks after adding a
        // new migration, updating the expected name here is a
        // deliberate signal to update migration docs.
        let last_name: String = conn
            .query_row(
                "SELECT name FROM _migrations ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            last_name, "0112_skin_hydra",
            "unexpected last migration name: {last_name}"
        );
    }

    #[test]
    fn feedback_canonical_kind_repair_flips_poison_sandbox_stamp() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();

        let project_a = "proj-a-0107";
        let project_b = "proj-b-0107";
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, 'A', '/tmp/a-0107'), (?2, 'B', '/tmp/b-0107')",
            params![project_a, project_b],
        )
        .unwrap();
        let canonical_id = "conv-canonical-0107";
        conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES ('ws-a', ?1, ?2, 'claude', 'user', 'sleeping', unixepoch())",
            params![project_a, canonical_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO feedback (id, project_id, session_id, session_kind, agent_name, kind, title, priority, status, created_at, updated_at) \
             VALUES ('fb-poison', ?1, ?2, 'sandbox', 'scout', 'question', 't', 3, 'waiting', 0, 0)",
            params![project_a, canonical_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feedback (id, project_id, session_id, session_kind, agent_name, kind, title, priority, status, created_at, updated_at) \
             VALUES ('fb-other-project', ?1, ?2, 'sandbox', 'scout', 'question', 't', 3, 'waiting', 0, 0)",
            params![project_b, canonical_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feedback (id, project_id, session_id, session_kind, agent_name, kind, title, priority, status, created_at, updated_at) \
             VALUES ('fb-true-sandbox', ?1, 'sess-random-sandbox', 'sandbox', 'scout', 'question', 't', 3, 'waiting', 0, 0)",
            params![project_a],
        )
        .unwrap();

        conn.execute_batch(include_str!(
            "../../drizzle_sql/0107_feedback_canonical_kind_repair.sql"
        ))
        .unwrap();

        let poison: String = conn
            .query_row(
                "SELECT session_kind FROM feedback WHERE id = 'fb-poison'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(poison, "canonical", "poison sandbox+canonical-id must flip");

        let other: String = conn
            .query_row(
                "SELECT session_kind FROM feedback WHERE id = 'fb-other-project'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            other, "sandbox",
            "same session_id on a different project_id must not flip"
        );

        let true_sandbox: String = conn
            .query_row(
                "SELECT session_kind FROM feedback WHERE id = 'fb-true-sandbox'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(true_sandbox, "sandbox", "random sandbox UUID must stay sandbox");

        conn.execute_batch(include_str!(
            "../../drizzle_sql/0107_feedback_canonical_kind_repair.sql"
        ))
        .unwrap();
        let poison2: String = conn
            .query_row(
                "SELECT session_kind FROM feedback WHERE id = 'fb-poison'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(poison2, "canonical");
    }

    #[test]
    fn seed_agent_presets_creates_expected_entries() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        seed_agent_presets(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_presets WHERE is_built_in = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 13, "expected 13 built-in presets");
        let cmd: String = conn
            .query_row(
                "SELECT command FROM agent_presets WHERE label = 'Codex' AND is_built_in = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cmd, "codex --yolo");
    }

    #[test]
    fn seed_agent_presets_rewrites_stock_codex_command_only() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES ('legacy-codex', 'Codex', \
                     'codex -c model_reasoning_effort=\"high\" --dangerously-bypass-approvals-and-sandbox', \
                     '', 1, 1, 1)",
            [],
        )
        .unwrap();
        seed_agent_presets(&conn).unwrap();
        let cmd: String = conn
            .query_row(
                "SELECT command FROM agent_presets WHERE label = 'Codex' AND is_built_in = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cmd, "codex --yolo");

        conn.execute(
            "UPDATE agent_presets SET command = 'codex --profile work' WHERE label = 'Codex'",
            [],
        )
        .unwrap();
        seed_agent_presets(&conn).unwrap();
        let cmd: String = conn
            .query_row(
                "SELECT command FROM agent_presets WHERE label = 'Codex' AND is_built_in = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            cmd, "codex --profile work",
            "must not clobber a user-customized Codex command"
        );
    }

    #[test]
    fn seed_agent_presets_idempotent_across_reseeds() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        seed_agent_presets(&conn).unwrap();
        seed_agent_presets(&conn).unwrap();
        seed_agent_presets(&conn).unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_presets WHERE is_built_in = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 13, "reseeding must not duplicate rows");
    }

    /// Migration-0070 metadata lands truthfully on a FRESH install: the
    /// five audited danger flags on their owners, the studied readiness
    /// classes, NULL (= honest unknown) everywhere else, env NULL for
    /// every built-in.
    #[test]
    fn seed_agent_presets_stamps_0070_metadata() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        seed_agent_presets(&conn).unwrap();

        let row = |label: &str| -> (Option<String>, Option<String>, Option<String>) {
            conn.query_row(
                "SELECT danger_flags, env, readiness FROM agent_presets \
                 WHERE label = ?1 AND is_built_in = 1",
                params![label],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
        };

        assert_eq!(
            row("Claude"),
            (
                Some(r#"["--dangerously-skip-permissions"]"#.into()),
                None,
                Some("bracketed-paste".into())
            )
        );
        assert_eq!(
            row("Codex"),
            (
                Some(r#"["--dangerously-bypass-approvals-and-sandbox"]"#.into()),
                None,
                Some("settle:2000".into())
            )
        );
        assert_eq!(
            row("Gemini"),
            (Some(r#"["--yolo"]"#.into()), None, Some("settle:2000".into()))
        );
        assert_eq!(
            row("Grok"),
            (Some(r#"["--always-approve"]"#.into()), None, Some("bracketed-paste".into()))
        );
        let grok_flow: Option<String> = conn
            .query_row(
                "SELECT inject_flow FROM agent_presets WHERE label = 'Grok' AND is_built_in = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            grok_flow.as_deref(),
            Some(crate::inject_flow::GROK_INJECT_FLOW_JSON)
        );
        assert_eq!(row("Cursor Agent"), (None, None, Some("bracketed-paste".into())));
        assert_eq!(row("Pi"), (None, None, Some("settle:1500".into())));
        assert_eq!(row("Hermes"), (None, None, Some("settle:7000".into())));
        assert_eq!(
            row("Copilot"),
            (Some(r#"["--allow-all"]"#.into()), None, None)
        );
        // The unaudited/unstudied built-ins stay honestly NULL.
        for label in ["OpenCode", "Goose", "Aider", "Ollama", "Interpreter"] {
            assert_eq!(row(label), (None, None, None), "{label} must stay NULL");
        }
        // Every danger_flags value must be VALID JSON (the resolver
        // parses these; a typo here would silently degrade to the floor).
        let mut stmt = conn
            .prepare("SELECT label, danger_flags FROM agent_presets WHERE danger_flags IS NOT NULL")
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(rows.len(), 5, "exactly the five audited flag owners");
        for (label, json) in rows {
            let parsed: Vec<String> = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{label} danger_flags must be a JSON string array: {e}"));
            assert!(!parsed.is_empty(), "{label} danger_flags must be non-empty");
        }
    }

    /// UPGRADE path: rows that pre-existed the 0070 ALTERs (metadata all
    /// NULL, possibly with a user-customized command) get the truthful
    /// backfill on the next reseed — and a non-NULL value is NEVER
    /// clobbered by later reseeds.
    #[test]
    fn seed_agent_presets_backfills_existing_rows_without_clobbering() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        // Simulate a pre-0070 installed row: present, customized command,
        // NULL metadata (exactly what the ALTERs leave behind).
        conn.execute(
            "INSERT INTO agent_presets (id, label, command, icon, enabled, sort_order, is_built_in) \
             VALUES ('legacy-claude-id', 'Claude', 'claude --model opus --dangerously-skip-permissions', '', 1, 0, 1)",
            [],
        )
        .unwrap();
        seed_agent_presets(&conn).unwrap();

        let (command, danger, readiness): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT command, danger_flags, readiness FROM agent_presets \
                 WHERE label = 'Claude' AND is_built_in = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            command, "claude --model opus --dangerously-skip-permissions",
            "seed must not clobber the user's customized command"
        );
        assert_eq!(danger.as_deref(), Some(r#"["--dangerously-skip-permissions"]"#));
        assert_eq!(readiness.as_deref(), Some("bracketed-paste"));

        // Non-NULL metadata survives reseeds (COALESCE semantics).
        conn.execute(
            "UPDATE agent_presets SET danger_flags = '[\"--custom-flag\"]' WHERE label = 'Claude'",
            [],
        )
        .unwrap();
        seed_agent_presets(&conn).unwrap();
        let danger: Option<String> = conn
            .query_row(
                "SELECT danger_flags FROM agent_presets WHERE label = 'Claude' AND is_built_in = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            danger.as_deref(),
            Some(r#"["--custom-flag"]"#),
            "reseed must never overwrite non-NULL metadata"
        );
    }

    /// 0072: junk heartbeat rows from the GH#22/#23/#24 CLI misparse
    /// (mode usually the valid 'scheduled' with a garbage $.frequency
    /// like "--help"; sometimes a junk mode outright) reset to
    /// off/NULL/disabled — while every legitimate shape survives
    /// untouched. Fresh installs run 0072 against an empty table, so
    /// the healing UPDATE is exercised here by re-applying the SQL to
    /// seeded rows (the statement is idempotent by construction).
    #[test]
    fn migration_0072_heals_junk_heartbeat_schedule_rows() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();

        let insert = |id: &str, mode: &str, schedule: Option<&str>, enabled: i64| {
            conn.execute(
                "INSERT INTO projects (id, name, path, heartbeat_mode, heartbeat_schedule, heartbeat_enabled) \
                 VALUES (?1, ?1, ?1, ?2, ?3, ?4)",
                params![id, mode, schedule, enabled],
            )
            .unwrap();
        };
        // The junk shapes actually observed (GH#22/#23/#24) + edge cases.
        insert("junk-freq", "scheduled", Some(r#"{"frequency":"--help","time":"09:00"}"#), 1);
        insert("junk-word", "scheduled", Some(r#"{"frequency":"add"}"#), 1);
        insert("junk-not-json", "scheduled", Some("--help"), 1);
        insert("junk-no-freq", "scheduled", Some(r#"{"time":"09:00"}"#), 1);
        insert("junk-null-schedule", "scheduled", None, 1);
        insert("junk-mode", "--help", Some(r#"{"frequency":"daily"}"#), 1);
        // Legitimate rows that must survive byte-identically.
        insert("keep-off", "off", None, 0);
        insert("keep-hourly", "hourly", Some(r#"{"start":"00:00","end":"23:59","every_seconds":300}"#), 1);
        insert("keep-weekly", "scheduled", Some(r#"{"frequency":"weekly","time":"09:00"}"#), 1);

        conn.execute_batch(include_str!(
            "../../drizzle_sql/0072_clear_junk_heartbeat_schedules.sql"
        ))
        .unwrap();

        let row = |id: &str| -> (String, Option<String>, i64) {
            conn.query_row(
                "SELECT heartbeat_mode, heartbeat_schedule, heartbeat_enabled \
                 FROM projects WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
        };
        for id in [
            "junk-freq",
            "junk-word",
            "junk-not-json",
            "junk-no-freq",
            "junk-null-schedule",
            "junk-mode",
        ] {
            assert_eq!(
                row(id),
                ("off".into(), None, 0),
                "{id} must be healed to off/NULL/disabled"
            );
        }
        assert_eq!(row("keep-off"), ("off".into(), None, 0));
        assert_eq!(
            row("keep-hourly"),
            (
                "hourly".into(),
                Some(r#"{"start":"00:00","end":"23:59","every_seconds":300}"#.into()),
                1
            )
        );
        assert_eq!(
            row("keep-weekly"),
            (
                "scheduled".into(),
                Some(r#"{"frequency":"weekly","time":"09:00"}"#.into()),
                1
            )
        );
    }

    /// 0073: the `session_provider` column lands on
    /// `workspace_heartbeats`, the migration is registered in
    /// `_migrations`, and a re-run is a no-op (a second ALTER of the
    /// same column would error — the `already_applied` guard is what
    /// makes this idempotent).
    #[test]
    fn migration_0073_adds_session_provider_column() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        let has: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('workspace_heartbeats') \
                 WHERE name = 'session_provider'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(has, 1, "workspace_heartbeats.session_provider must exist");
        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM _migrations WHERE name = '0073_heartbeat_session_provider'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1, "0073 must be registered exactly once");
        // Re-run: must not attempt the ALTER again (would error).
        run_migrations(&conn).unwrap();
    }

    /// 0074: the `subdomain_workspaces` attribution table exists with
    /// its two columns (`label` PK, `project_id` NOT NULL), the
    /// migration is registered in `_migrations`, and a re-run is a
    /// no-op (CREATE TABLE IF NOT EXISTS + the `already_applied`
    /// guard both keep it idempotent).
    #[test]
    fn migration_0074_creates_subdomain_workspaces_table() {
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        let cols: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('subdomain_workspaces') \
                 WHERE name IN ('label', 'project_id')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cols, 2, "subdomain_workspaces must have label + project_id");
        let applied: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM _migrations WHERE name = '0074_subdomain_workspaces'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(applied, 1, "0074 must be registered exactly once");
        // `label` is the PK — a second claim of the same label must
        // REPLACE, not duplicate (the claim seam relies on this).
        conn.execute(
            "INSERT OR REPLACE INTO subdomain_workspaces (label, project_id) VALUES ('staging', 'p1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO subdomain_workspaces (label, project_id) VALUES ('staging', 'p2')",
            [],
        )
        .unwrap();
        let (n, pid): (i64, String) = conn
            .query_row(
                "SELECT COUNT(*), MAX(project_id) FROM subdomain_workspaces WHERE label = 'staging'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((n, pid.as_str()), (1, "p2"), "PK upsert must repoint, not duplicate");
        run_migrations(&conn).unwrap();
    }

    // ── purge_orphan_project_children self-heal ───────────────────
    #[test]
    fn purge_orphan_project_children_removes_stranded_rows() {
        // Reproduces the 0.37.0-launch crash: a DB with rows in
        // FK-bearing tables whose parent `projects` row was deleted
        // under earlier versions where FK enforcement was off.
        // Without the self-heal, migration 0039's
        // `INSERT INTO workspace_sessions … SELECT … FROM agent_sessions`
        // would trip the new `REFERENCES projects(id)` constraint.
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();

        // Seed two projects + child rows for both, then delete
        // ONE project bypassing CASCADE (FK off, simulating the
        // pre-0.37.0 code path that left the orphans).
        conn.execute(
            "INSERT INTO projects (id, path, name) VALUES \
             ('keep-me', '/tmp/keep', 'keep'), \
             ('orphan-me', '/tmp/orphan', 'orphan')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO heartbeat_fires (id, project_id, agent_name, fired_at, mode, decision) \
             VALUES (1, 'keep-me',   'a', '2026-05-06', 'manual', 'fired'), \
                    (2, 'orphan-me', 'a', '2026-05-06', 'manual', 'fired'), \
                    (3, 'orphan-me', 'a', '2026-05-06', 'manual', 'fired')",
            [],
        )
        .unwrap();
        // activity_feed schema: id INTEGER, project_id, event_type, summary, metadata...
        // Use AUTOINCREMENT for id; just insert event_type + summary.
        conn.execute(
            "INSERT INTO activity_feed (project_id, event_type, summary) \
             VALUES ('keep-me',   'message.sent', 'kept'), \
                    ('orphan-me', 'message.sent', 'orphan-1'), \
                    ('orphan-me', 'message.sent', 'orphan-2')",
            [],
        )
        .unwrap();

        // Delete the orphan project with FK off — what older
        // versions effectively did.
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute(
            "DELETE FROM projects WHERE id = 'orphan-me'",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        // Confirm the orphans are present (the bug we're fixing).
        let orphan_fires: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM heartbeat_fires WHERE project_id = 'orphan-me'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_fires, 2, "test setup should produce 2 orphan fires");

        // Run the self-heal.
        purge_orphan_project_children(&conn).unwrap();

        // Orphans gone, kept-project rows preserved.
        let remaining_orphan_fires: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM heartbeat_fires WHERE project_id = 'orphan-me'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let kept_fires: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM heartbeat_fires WHERE project_id = 'keep-me'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining_orphan_fires, 0, "orphan heartbeat_fires must be purged");
        assert_eq!(kept_fires, 1, "non-orphan rows must be preserved");

        let remaining_orphan_af: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM activity_feed WHERE project_id = 'orphan-me'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining_orphan_af, 0, "orphan activity_feed rows must be purged");

        // PRAGMA foreign_key_check should report clean.
        let fk_violations: Vec<String> = conn
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(
            fk_violations.is_empty(),
            "foreign_key_check should be clean after self-heal, got: {fk_violations:?}"
        );
    }

    #[test]
    fn purge_orphan_project_children_idempotent_on_clean_db() {
        // Running the sweep against a freshly-migrated DB with no
        // orphans must be a no-op — it'll fire on every K2SO launch
        // post-0.37.1, so it has to stay cheap and harmless.
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();
        purge_orphan_project_children(&conn).unwrap();
        purge_orphan_project_children(&conn).unwrap();
        // Re-running shouldn't fail or re-introduce any rows.
    }

    #[test]
    fn purge_orphan_project_children_handles_pre_migration_db() {
        // Bare-minimum DB without any FK-bearing tables — sweep must
        // return Ok cleanly so it can run BEFORE migrations on a
        // brand-new install.
        let conn = Connection::open(":memory:").unwrap();
        // No projects table, no child tables — projects_exists check
        // should short-circuit.
        purge_orphan_project_children(&conn).unwrap();
    }

    // ── open_with_resilience PRAGMAs ──────────────────────────────
    #[test]
    fn open_with_resilience_sets_foreign_keys_on() {
        let path = scratch_db_path();
        let conn = open_with_resilience(&path).unwrap();
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys should be ON after open");
        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn open_with_resilience_sets_wal_mode_on_disk_db() {
        // journal_mode=WAL only sticks on file-backed DBs; memory DBs
        // report "memory". That's why this test uses a disk path.
        let path = scratch_db_path();
        let conn = open_with_resilience(&path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "expected WAL mode, got {}", mode);
        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn open_with_resilience_sets_pragmas() {
        let path = scratch_db_path();
        let conn = open_with_resilience(&path).unwrap();
        // busy_timeout: 500ms as of 0.32.13 (was 5000 — masked real contention
        // behind a 5 s UI hang).
        let timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert_eq!(timeout, 500, "busy_timeout should be 500ms");

        // cache_size negative means KiB (positive means pages). -20000 = 20 MB.
        let cache_size: i64 = conn
            .query_row("PRAGMA cache_size", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cache_size, -20000, "cache_size should be -20000 (20MB)");

        // temp_store 2 = MEMORY (0=default, 1=FILE, 2=MEMORY).
        let temp_store: i64 = conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .unwrap();
        assert_eq!(temp_store, 2, "temp_store should be 2 (MEMORY)");

        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    // ── bootstrap_test_db_at ──────────────────────────────────────
    #[test]
    fn bootstrap_test_db_at_creates_usable_database() {
        let path = scratch_db_path();
        bootstrap_test_db_at(&path).unwrap();

        // Reopen and verify tables + presets present.
        let conn = open_with_resilience(&path).unwrap();
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_presets'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);
        let preset_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_presets WHERE is_built_in=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(preset_count, 13);
        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn bootstrap_test_db_at_is_rerunnable_on_existing_file() {
        // If the user (or a prior test run) left a DB file in place,
        // bootstrap_test_db_at must still succeed without duplicating
        // rows or failing migrations.
        let path = scratch_db_path();
        bootstrap_test_db_at(&path).unwrap();
        bootstrap_test_db_at(&path).unwrap();
        let conn = open_with_resilience(&path).unwrap();
        let presets: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_presets WHERE is_built_in=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(presets, 13, "re-bootstrap must not duplicate presets");
        drop(conn);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    // ── isolated_test_connection ──────────────────────────────────
    #[test]
    fn isolated_test_connection_gives_distinct_databases() {
        // Two calls to isolated_test_connection return two different
        // :memory: connections — a write to one must not be visible
        // from the other. This is the isolation guarantee that lets
        // unit tests run in parallel without polluting each other.
        let a = isolated_test_connection();
        let b = isolated_test_connection();

        // Insert a project row into A via raw SQL (bypassing schema::
        // helpers so we don't need a project_id generator).
        a.execute(
            "INSERT INTO projects (id, name, path) VALUES ('p-iso', 'a', '/iso')",
            [],
        )
        .unwrap();

        let a_has: i64 = a
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE id='p-iso'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let b_has: i64 = b
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE id='p-iso'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(a_has, 1, "A must see its own write");
        assert_eq!(b_has, 0, "B must not see A's write");
    }

    #[test]
    fn isolated_test_connection_carries_full_schema() {
        // Spot-check: every table hit by schema::unit_tests must be
        // present in a fresh isolated_test_connection.
        let conn = isolated_test_connection();
        for table in [
            "projects",
            "workspace_sessions",
            "workspace_heartbeats",
            "agent_presets",
            "heartbeat_fires",
            "activity_feed",
            "workspace_relations",
            "focus_groups",
            "published_services",
            "overlay_conversations",
            "overlay_host",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "isolated connection missing table: {}", table);
        }
    }

    // ── Migration 0049: __lead__ sentinel rewrite ─────────────────
    #[test]
    fn migration_0049_rewrites_lead_sentinel_for_manager_workspaces() {
        // Build a DB at migration 0048 (the state before 0049 ships),
        // seed `activity_feed` with rows addressed to the pre-
        // unification `__lead__` routing sentinel for two workspaces
        // (one in manager mode, one in custom mode), run the full
        // migration sequence (which includes 0049), and assert each
        // row landed on the correct post-cleanup `to_workspace`:
        //
        //   - manager-mode workspace → `to_workspace = projects.name`
        //   - custom-mode workspace  → `to_workspace = NULL`
        let conn = fresh_memory();
        run_migrations(&conn).unwrap();

        // Two workspaces.
        conn.execute(
            "INSERT INTO projects (id, name, path, agent_mode) VALUES \
             ('proj-mgr', 'manager-ws', '/tmp/mgr', 'manager'), \
             ('proj-cus', 'custom-ws',  '/tmp/cus', 'custom')",
            [],
        )
        .unwrap();

        // Backdate the migration row + clear it so we can re-run 0049
        // on legacy data.
        conn.execute(
            "DELETE FROM _migrations WHERE name = '0049_drop_lead_sentinel_in_activity_feed'",
            [],
        )
        .unwrap();

        // Seed pre-0.39.0f rows in both workspaces, plus a control
        // row that's already addressed correctly (must be untouched).
        conn.execute(
            "INSERT INTO activity_feed \
             (project_id, actor, event_type, from_workspace, to_workspace, summary) VALUES \
             ('proj-mgr', 'sender', 'message.sent', 'sender', '__lead__', 'mgr msg'), \
             ('proj-cus', 'sender', 'message.sent', 'sender', '__lead__', 'cus msg'), \
             ('proj-mgr', 'sender', 'message.sent', 'sender', 'manager-ws', 'already correct')",
            [],
        )
        .unwrap();

        // Re-run migrations — 0049 fires.
        run_migrations(&conn).unwrap();

        // Manager workspace's sentinel row → projects.name.
        let mgr_target: Option<String> = conn
            .query_row(
                "SELECT to_workspace FROM activity_feed WHERE summary = 'mgr msg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            mgr_target.as_deref(),
            Some("manager-ws"),
            "manager workspace's __lead__ row should rewrite to projects.name"
        );

        // Custom workspace's sentinel row → NULL (no primary to route to).
        let cus_target: Option<String> = conn
            .query_row(
                "SELECT to_workspace FROM activity_feed WHERE summary = 'cus msg'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            cus_target.is_none(),
            "non-manager workspace's __lead__ row should null out, got: {:?}",
            cus_target
        );

        // Control row untouched.
        let control: Option<String> = conn
            .query_row(
                "SELECT to_workspace FROM activity_feed WHERE summary = 'already correct'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(control.as_deref(), Some("manager-ws"));

        // Sanity: no row anywhere still says `'__lead__'`.
        let leftover: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM activity_feed WHERE to_workspace = '__lead__'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leftover, 0, "no row should retain the __lead__ sentinel");
    }

    // ── Migration 0033 tests deleted in 0.37.0 ────────────────────
    //
    // Migration 0033 (agent_session terminal_id namespace, 0.36.0)
    // rewrote legacy `agent-chat-<agent>` terminal_ids to the
    // workspace-scoped `agent-chat:<project_id>:<agent>` form. The
    // migration ran exactly once on every existing user's DB and is
    // historical now. Migration 0039 (0.37.0) renames the underlying
    // table from `agent_sessions` to `workspace_sessions` and drops
    // `agent_name`, so the test substrate (seed/read against the old
    // table) no longer exists. The 0033 SQL still runs on each fresh
    // DB during the migration sequence — it just operates on rows
    // that 0039 immediately collapses + table-renames a few steps
    // later. Equivalent regression coverage for the current shape
    // lives in `schema::tests::workspace_session_*`.
}
