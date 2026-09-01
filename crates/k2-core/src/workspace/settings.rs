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

/// B2 (0.40.24) — map the CLI-canonical agent mode `"k2"` onto the
/// stored legacy spelling `"k2so"`.
///
/// The CLI's documented mode vocabulary is `off | custom | manager | k2`
/// (cli/k2 validates exactly that), but every stored-value consumer —
/// the canonical-spawn check in the daemon's `/cli/mode` route, the
/// `agent_identity` type resolver, wake-prompt templates, the 0.37.0
/// unification migration — matches the legacy `"k2so"` spelling. Rather
/// than migrate every reader + existing DB rows, we normalize at the
/// WRITE boundary: both spellings are accepted as input, one spelling
/// is ever stored. Read-side display normalization is the inverse —
/// see [`display_agent_mode`].
pub fn stored_agent_mode_value(mode: &str) -> &str {
    if mode == "k2" {
        "k2so"
    } else {
        mode
    }
}

/// B2 (0.40.24) — inverse of [`stored_agent_mode_value`]: map the stored
/// legacy spellings (`"k2so"`, and the UI's historic `"agent"` synonym —
/// `agent_identity::agent_type_for` treats both as the same mode) onto
/// the CLI-canonical `"k2"` for display. Every other value passes
/// through untouched. Used by read surfaces (`k2 agent conf/get`) so
/// operators only ever see the documented vocabulary.
pub fn display_agent_mode(stored: &str) -> &str {
    match stored {
        // Stage A: both spellings (plus the UI historic `agent` synonym)
        // display as the CLI-canonical `k2`.
        "k2so" | "k2" | "agent" => "k2",
        other => other,
    }
}

/// The `projects` columns [`update_project_setting`] may write. Public
/// so route-level writers (the daemon's `/cli/workspace/set`) can
/// validate a whole field batch up front — rejecting the entire request
/// before applying anything — instead of failing halfway through.
pub fn allowed_project_setting_fields() -> &'static [&'static str] {
    &[
        "agent_mode",
        "worktree_mode",
        "heartbeat_enabled",
        "agent_enabled",
        "pinned",
        // tier_id (Workspace States) retired — no longer writable via settings.
        // 0.34.0 Session Stream opt-in (Phase 2). Values: 'on' | 'off'.
        "use_session_stream",
        // #67 — per-workspace remote-instruct opt-in. Values: '1' | '0'
        // (default 0/OFF, fail-closed). Gates the composer's connect-user
        // path for THIS workspace; see `remote_instruct_allowed_for_path`.
        "allow_remote_instruct",
        // DNS K1 — per-workspace DNS-manage opt-in. Values: '1' | '0'
        // (default 0/OFF, fail-closed). Gates agent DNS mutation for
        // THIS workspace; see `dns_manage_allowed_for_path`.
        "dns_manage_enabled",
        // C1 (0.40.45) — per-workspace agents-may-create-connections
        // opt-in. Values: '1' | '0' (default 0/OFF, fail-closed). Gates
        // agent `connections add|remove` for THIS workspace; see
        // `agents_can_create_connections_for_path`. Owner always bypasses.
        "agents_can_create_connections",
        // Sandbox v2 (PRD §G2 #1) — per-workspace sandbox FS mode. Values:
        // 'overlay' | 'ro+scratch' (default 'overlay'). See `get_workspace_fs_mode`.
        "sandbox_fs_mode",
        // Host sessions F1 (prd-v1-api-completion §3) — whether API-spawned
        // HOST sessions keep the agent preset's dangerous auto-approve flags.
        // Values: '1' | '0'. Product default ON (headless /v1); '0' = opt-out.
        // See `get_api_skip_permissions` + migration 0093.
        "api_skip_permissions",
        // W6 (0.40.30) — per-workspace default agent (0063 column): an
        // `agent_presets` preset id, or a legacy command first-token.
        // Stored VERBATIM, same tolerance as `/cli/projects/update`'s
        // `defaultAgent` key — the resolver (`workspace::agent_resolve`)
        // skips unresolvable/disabled values and falls through, so a
        // stale value can never brick a spawn. `k2 agent hire --agent`
        // writes this via `/cli/workspace/set` (validating the preset
        // CLI-side first).
        "default_agent",
        // K2 Mail (prd-email-server-v1 §12 / D4) — per-workspace
        // agent-send gating override. Values: 'off' | 'approval' | 'on'
        // (write-validated below); NULL = inherit the global
        // `AppSettings.mail_agent_send` default. Effective resolver:
        // `mail_agent_send_for_path` (fail-closed to 'off').
        "mail_agent_send",
        // K2 Mail (prd-email-server-v1 §12 / D6) — per-workspace
        // address-cap override. Non-negative integer, 0 = unlimited
        // (write-validated below); NULL = inherit the global
        // `AppSettings.mail_address_cap` default (5). Effective
        // resolver: `mail_address_cap_for_path`.
        "mail_address_cap",
        // Workspace data sidecar (prd-workspace-data-sidecar-v1 D21) —
        // create-only agent passport (`k2 db create`). Values:
        // 'off' | 'read' | 'write' (write-validated below); NULL =
        // fail-closed 'off'. List/dsn/store use ownership or sql_grants.
        "db_agent_access",
        // D9: per-workspace ACTIVE-DB cap. Non-negative integer, 0 =
        // unlimited (write-validated below); NULL = inherit default 1.
        "db_active_cap",
        // Phase 0b (prd-wiki-public-chat-api-loopback-v1) — per-workspace
        // owner API guest policy text. Free-form; empty/NULL → platform
        // default. Injected by the daemon on every host-session spawn +
        // message-live; callers cannot set it from the request body.
        // See `get_api_guest_policy` / `DEFAULT_API_GUEST_POLICY`.
        "api_guest_policy",
        // Phase 1 (prd-wiki-public-chat-api-loopback-v1) — per-workspace
        // public wiki chat opt-in. Values: '1' | '0' (default 0/OFF).
        // Serve alone never enables chat (D6). See `get_wiki_public_chat`.
        "wiki_public_chat",
        // Per-workspace concurrent host-session (live cell) cap.
        // Positive integer 1..=MAX_HOST_SESSION_CELL_CAP (512); empty /
        // "default" / "null" → store NULL (inherit daemon default via
        // env K2_SANDBOX_WORKSPACE_CELL_CAP or 15). See
        // `get_host_session_cell_cap`.
        "host_session_cell_cap",
        // Hide auto-surfaced API host-session / sandbox tabs (default 0).
        // Sessions remain listed in Chat history → API.
        "hide_api_sessions",
        // Per-workspace completion chime (default 1 / ON). AND-gated
        // with the global Settings → General toggle in the renderer.
        "completion_sound_enabled",
        // 0106 — per-workspace default model (opaque harness id). Empty → NULL.
        "default_model",
        // 0106 — splice workspace default on dead resume. Values: '0' | '1'.
        "force_model_on_resume",
    ]
}

/// Daemon ceiling for a per-workspace host-session concurrent cell cap.
/// Agents can raise the cap via CLI / workspace set, but not past this.
pub const MAX_HOST_SESSION_CELL_CAP: usize = 512;

/// Product default concurrent host-session cells per workspace when the
/// column is NULL and env `K2_SANDBOX_WORKSPACE_CELL_CAP` is unset.
/// Must stay aligned with `sandbox_quota::DEFAULT_WORKSPACE_CELL_CAP`.
pub const DEFAULT_HOST_SESSION_CELL_CAP: usize = 15;

/// Platform default when `projects.api_guest_policy` is NULL or blank.
/// Byte-stable — host-session inject + tests pin it. Soft framing only;
/// never replaces hard capability grants.
pub const DEFAULT_API_GUEST_POLICY: &str = "\
[K2 API guest policy] The external API client is NOT the workspace owner. \
Prefer read-only work; do not modify files, wiki notes, or secrets unless a \
tool explicitly allows it for this call. Report progress with \
`k2 respond '…'` and the final answer with `k2 respond --final '…'` — only \
those lines reach the caller.";

/// The valid `mail_agent_send` gating modes (PRD §8.4 / D4).
pub const MAIL_AGENT_SEND_MODES: [&str; 3] = ["off", "approval", "on"];

/// The valid `db_agent_access` passport modes (prd-workspace-data-sidecar-v1 D21).
pub const DB_AGENT_ACCESS_MODES: [&str; 3] = ["off", "read", "write"];

/// Default ACTIVE workspace-DB cap when `projects.db_active_cap` is NULL (D9).
pub const DEFAULT_DB_ACTIVE_CAP: u32 = 1;

/// Update a single project setting. Field names are allowlisted —
/// the SQL interpolates the column name directly so any arbitrary
/// string from query params would be an injection vector without
/// this check.
pub fn update_project_setting(
    project_path: &str,
    field: &str,
    value: &str,
) -> Result<(), String> {
    // B2 (0.40.24): normalize the CLI-canonical mode spelling onto the
    // stored one so `k2 settings --mode k2` / `k2 agent set --mode k2`
    // land on the value every downstream reader matches.
    let value = if field == "agent_mode" {
        stored_agent_mode_value(value)
    } else {
        value
    };

    let db = crate::db::shared();
    let conn = db.lock();

    let allowed = allowed_project_setting_fields();
    if !allowed.contains(&field) {
        return Err(format!("Unknown setting: {}", field));
    }
    // Sandbox v2: validate the FS mode is exactly one of the two first-class
    // values so a typo can never be STORED (read-side fails safe to 'overlay',
    // but we still reject a bad WRITE loudly rather than silently coercing).
    if field == "sandbox_fs_mode" && value != "overlay" && value != "ro+scratch" {
        return Err(format!(
            "sandbox_fs_mode must be 'overlay' or 'ro+scratch', got {value:?}"
        ));
    }
    // Validate the per-workspace remote-instruct flag so a typo can't
    // silently leave a security gate in an undefined state. Stored as a
    // 0/1 int column (migration 0054).
    if field == "allow_remote_instruct" && value != "0" && value != "1" {
        return Err(format!(
            "allow_remote_instruct must be '0' or '1', got {value:?}"
        ));
    }
    // DNS K1 — same discipline for the per-workspace DNS-manage flag
    // (migration 0079). Stored as a 0/1 int column.
    if field == "dns_manage_enabled" && value != "0" && value != "1" {
        return Err(format!(
            "dns_manage_enabled must be '0' or '1', got {value:?}"
        ));
    }
    // C1 (0.40.45) — same discipline for the per-workspace
    // agents-may-create-connections flag (migration 0085).
    if field == "agents_can_create_connections" && value != "0" && value != "1" {
        return Err(format!(
            "agents_can_create_connections must be '0' or '1', got {value:?}"
        ));
    }
    // Same discipline for the host-session skip-permissions opt-in
    // (migration 0069) — it gates dangerous auto-approve flags, so a typo
    // must never leave it in an undefined state.
    if field == "api_skip_permissions" && value != "0" && value != "1" {
        return Err(format!(
            "api_skip_permissions must be '0' or '1', got {value:?}"
        ));
    }
    // Phase 1 — public wiki chat opt-in (migration 0088). Security-adjacent
    // exposure flag; reject non 0/1 so a typo never leaves an undefined gate.
    if field == "wiki_public_chat" && value != "0" && value != "1" {
        return Err(format!(
            "wiki_public_chat must be '0' or '1', got {value:?}"
        ));
    }
    if field == "hide_api_sessions" && value != "0" && value != "1" {
        return Err(format!(
            "hide_api_sessions must be '0' or '1', got {value:?}"
        ));
    }
    if field == "completion_sound_enabled" && value != "0" && value != "1" {
        return Err(format!(
            "completion_sound_enabled must be '0' or '1', got {value:?}"
        ));
    }
    if field == "force_model_on_resume" && value != "0" && value != "1" {
        return Err(format!(
            "force_model_on_resume must be '0' or '1', got {value:?}"
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
    // K2 Mail (D4): the send gate is a security gate — a typo must
    // never leave it in an undefined state (the read side would fail
    // closed to 'off', but reject the bad WRITE loudly too).
    if field == "mail_agent_send" && !MAIL_AGENT_SEND_MODES.contains(&value) {
        return Err(format!(
            "mail_agent_send must be 'off', 'approval', or 'on', got {value:?}"
        ));
    }
    // K2 Mail (D6): the cap must parse as a non-negative integer
    // (0 = unlimited) so the minting check never reads garbage.
    if field == "mail_address_cap" && value.parse::<u32>().is_err() {
        return Err(format!(
            "mail_address_cap must be a non-negative integer (0 = unlimited), got {value:?}"
        ));
    }
    if field == "db_agent_access" && !DB_AGENT_ACCESS_MODES.contains(&value) {
        return Err(format!(
            "db_agent_access must be 'off', 'read', or 'write', got {value:?}"
        ));
    }
    if field == "db_active_cap" && value.parse::<u32>().is_err() {
        return Err(format!(
            "db_active_cap must be a non-negative integer (0 = unlimited), got {value:?}"
        ));
    }
    // Per-workspace host-session cell cap: empty / "default" / "null"
    // clears to NULL (inherit daemon default). Otherwise require a
    // positive integer in 1..=MAX_HOST_SESSION_CELL_CAP — reject 0 and
    // non-numeric loudly; reject values above the daemon ceiling.
    if field == "host_session_cell_cap" {
        let trimmed = value.trim();
        let clear = trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("default")
            || trimmed.eq_ignore_ascii_case("null");
        if !clear {
            match trimmed.parse::<usize>() {
                Ok(0) => {
                    return Err(
                        "host_session_cell_cap must be >= 1 (use 'default' to inherit daemon default)"
                            .into(),
                    );
                }
                Ok(n) if n > MAX_HOST_SESSION_CELL_CAP => {
                    return Err(format!(
                        "host_session_cell_cap max is {MAX_HOST_SESSION_CELL_CAP} (daemon ceiling), got {n}"
                    ));
                }
                Ok(_) => {}
                Err(_) => {
                    return Err(format!(
                        "host_session_cell_cap must be a positive integer 1..={MAX_HOST_SESSION_CELL_CAP}, or 'default' to inherit, got {value:?}"
                    ));
                }
            }
        }
    }

    // host_session_cell_cap clear path stores SQL NULL (Option::None).
    let rows = if field == "host_session_cell_cap" {
        let trimmed = value.trim();
        let clear = trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("default")
            || trimmed.eq_ignore_ascii_case("null");
        if clear {
            conn.execute(
                "UPDATE projects SET host_session_cell_cap = NULL WHERE path = ?1",
                rusqlite::params![project_path],
            )
            .map_err(|e| format!("DB update failed: {}", e))?
        } else {
            // Validated positive integer above; store as integer.
            let n: i64 = trimmed.parse().expect("validated positive integer");
            conn.execute(
                "UPDATE projects SET host_session_cell_cap = ?1 WHERE path = ?2",
                rusqlite::params![n, project_path],
            )
            .map_err(|e| format!("DB update failed: {}", e))?
        }
    } else if field == "default_model" {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            conn.execute(
                "UPDATE projects SET default_model = NULL WHERE path = ?1",
                rusqlite::params![project_path],
            )
            .map_err(|e| format!("DB update failed: {}", e))?
        } else {
            conn.execute(
                "UPDATE projects SET default_model = ?1 WHERE path = ?2",
                rusqlite::params![trimmed, project_path],
            )
            .map_err(|e| format!("DB update failed: {}", e))?
        }
    } else {
        let sql = format!("UPDATE projects SET {} = ?1 WHERE path = ?2", field);
        conn.execute(&sql, rusqlite::params![value, project_path])
            .map_err(|e| format!("DB update failed: {}", e))?
    };

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
                pinned, name, use_session_stream, allow_remote_instruct, \
                dns_manage_enabled, agents_can_create_connections, \
                api_guest_policy, wiki_public_chat, api_skip_permissions, \
                host_session_cell_cap, hide_api_sessions, completion_sound_enabled, \
                default_model, force_model_on_resume \
         FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| {
            // `use_session_stream` landed in migration 0032 with
            // default 'off'; expose as a bool for React consumers
            // (matching every other toggle shape in this struct).
            let uss_raw = row
                .get::<_, Option<String>>(6)
                .unwrap_or(None)
                .unwrap_or_else(|| "off".to_string());
            // Phase 0b — effective guest policy (default when NULL/blank).
            let guest_raw = row
                .get::<_, Option<String>>(10)
                .unwrap_or(None)
                .unwrap_or_default();
            let api_guest_policy = if guest_raw.trim().is_empty() {
                DEFAULT_API_GUEST_POLICY.to_string()
            } else {
                guest_raw
            };
            // Host-sessions: NULL / missing → default ON (0093).
            let api_skip = match row.get::<_, Option<i64>>(12).unwrap_or(None) {
                None => true,
                Some(v) => v != 0,
            };
            // Per-workspace host-session cell cap: NULL = inherit daemon
            // default (expose as JSON null). Positive stored value is
            // clamped to the daemon ceiling for display.
            let host_cap_json = match row.get::<_, Option<i64>>(13).unwrap_or(None) {
                Some(v) if v >= 1 => {
                    let n = (v as usize).min(MAX_HOST_SESSION_CELL_CAP);
                    serde_json::json!(n)
                }
                _ => serde_json::Value::Null,
            };
            let hide_api = row.get::<_, i64>(14).unwrap_or(0) == 1;
            let completion_sound = row.get::<_, i64>(15).unwrap_or(1) != 0;
            let default_model = row
                .get::<_, Option<String>>(16)
                .unwrap_or(None)
                .and_then(|s| {
                    let t = s.trim().to_string();
                    if t.is_empty() { None } else { Some(t) }
                });
            let force_model_on_resume = row.get::<_, i64>(17).unwrap_or(0) != 0;
            Ok(serde_json::json!({
                "mode": row.get::<_, String>(0).unwrap_or_else(|_| "off".to_string()),
                "worktreeMode": row.get::<_, i64>(1).unwrap_or(0) == 1,
                "heartbeatEnabled": row.get::<_, i64>(2).unwrap_or(0) == 1,
                "agentEnabled": row.get::<_, i64>(3).unwrap_or(0) == 1,
                "pinned": row.get::<_, i64>(4).unwrap_or(0) == 1,
                "name": row.get::<_, String>(5).unwrap_or_default(),
                // Workspace States retired — stateId no longer exposed.
                "useSessionStream": uss_raw == "on",
                // #67 — per-workspace remote-instruct opt-in (default 0/OFF).
                "allowRemoteInstruct": row.get::<_, i64>(7).unwrap_or(0) == 1,
                // DNS K1 — per-workspace DNS-manage opt-in (default 0/OFF).
                "dnsManageEnabled": row.get::<_, i64>(8).unwrap_or(0) == 1,
                // C1 — per-workspace agents-may-create-connections (default 0/OFF).
                "agentsCanCreateConnections": row.get::<_, i64>(9).unwrap_or(0) == 1,
                // Phase 0b — effective API guest policy (platform default if unset).
                "apiGuestPolicy": api_guest_policy,
                // Phase 1 — public wiki chat opt-in (default OFF).
                "wikiPublicChat": row.get::<_, i64>(11).unwrap_or(0) == 1,
                // Host-sessions F1 — keep auto-approve on /v1 spawns (default ON).
                "apiSkipPermissions": api_skip,
                // Concurrent host-session cells for this workspace (null = inherit).
                "hostSessionCellCap": host_cap_json,
                // Hide auto-surfaced API session tabs (default off).
                "hideApiSessions": hide_api,
                // Per-workspace completion chime (default ON).
                "completionSoundEnabled": completion_sound,
                "defaultModel": default_model,
                "forceModelOnResume": force_model_on_resume,
            }))
        },
    )
    .map_err(|e| format!("Project not found: {}", e))
}

/// Agentic systems are always on (GA). The stored field remains for
/// settings.json / `/cli/agentic` wire compatibility, but readers never
/// branch on it to hide features.
///
/// **0.39.0 migration:** these accessors used to read the SQLite
/// `app_settings (key, value)` table created by migration 0050. That
/// table is now dead-but-inert (kept in the migration ladder for
/// rollback safety only); the canonical store is the JSON file.
pub fn get_agentic_enabled() -> bool {
    true
}

/// Persist `agenticSystemsEnabled: true` only. Off is rejected — the
/// product no longer supports disabling agentic systems. Callers that
/// pass `false` still succeed (no error) but the stored value stays on
/// so older clients cannot turn the feature off.
pub fn set_agentic_enabled(_enabled: bool) -> Result<(), String> {
    crate::app_settings::update(serde_json::json!({
        "agenticSystemsEnabled": true,
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

/// Host sessions F1 (prd-v1-api-completion §3) — whether API-spawned HOST
/// sessions for `project_path` keep dangerous auto-approve flags.
///
/// **Default ON** (prd-api-skip-permissions-default-on-v1): `/v1` is headless
/// — a HITL permission gate with no human stalls the session. Semantics:
///
/// - No project row → `false` (unknown path still fail-closed).
/// - Column `NULL` → `true` (unset = product default ON; 0093 backfills NULL→1).
/// - `1` → `true`.
/// - `0` → `false` (owner opt-out via CLI / workspace set).
///
/// When OFF the host-session policy resolver STRIPS known auto-approve flags
/// (`--dangerously-skip-permissions`, …) from the resolved agent command.
pub fn get_api_skip_permissions(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    match conn.query_row(
        "SELECT api_skip_permissions FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, Option<i64>>(0),
    ) {
        // Unknown / unregistered path — fail closed.
        Err(_) => false,
        // NULL column (pre-0093 residual or never written) → default ON.
        Ok(None) => true,
        Ok(Some(v)) => v != 0,
    }
}

/// Phase 0b — EFFECTIVE API guest policy for `project_path`.
///
/// Returns the owner-configured text when present and non-blank; otherwise
/// [`DEFAULT_API_GUEST_POLICY`]. Unknown/unregistered workspaces also get
/// the platform default (soft framing still applies; hard grants are
/// separate). Never reads caller request bodies — host-session inject is
/// the only consumer.
pub fn get_api_guest_policy(project_path: &str) -> String {
    let stored: Option<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT api_guest_policy FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    };
    match stored {
        Some(s) if !s.trim().is_empty() => s,
        _ => DEFAULT_API_GUEST_POLICY.to_string(),
    }
}

/// Raw stored `api_guest_policy` (None / empty = using platform default).
/// Used by CLI `get` so operators can tell custom vs default.
pub fn get_api_guest_policy_raw(project_path: &str) -> Option<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT api_guest_policy FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
    .and_then(|s| {
        if s.trim().is_empty() {
            None
        } else {
            Some(s)
        }
    })
}

/// Phase 1 — read the PER-WORKSPACE public wiki chat opt-in for `project_path`.
///
/// Returns `false` (fail-closed) when the project isn't registered or the
/// column reads NULL. Default OFF (D6): serve alone never enables chat.
/// Orthogonal to serve state — chat can be opted in while serve is off;
/// Phase 2 gateway still requires serve + this flag.
pub fn get_wiki_public_chat(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT wiki_public_chat FROM projects WHERE path = ?1",
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

/// DNS K1 — read the PER-WORKSPACE DNS-manage opt-in for `project_path`.
/// Returns `false` (fail-closed) when the project isn't registered or the
/// column reads NULL. Does NOT consider the app-level master flag — use
/// [`dns_manage_allowed_for_path`] for the effective gate decision.
pub fn get_dns_manage_enabled(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT dns_manage_enabled FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, i64>(0),
    )
    .map(|v| v == 1)
    .unwrap_or(false)
}

/// DNS K1 — the EFFECTIVE DNS-manage gate decision for `project_path`.
///
/// A workspace may manage DNS records iff its per-workspace flag is set
/// OR the app-level `dnsManageEnabled` master is on. The app-level flag
/// is a GLOBAL MASTER: enabling it once opts in every workspace, while
/// the default (both OFF) denies, fail-closed.
///
/// Fail-closed: an unknown/unregistered workspace + app-level off → false.
pub fn dns_manage_allowed_for_path(project_path: &str) -> bool {
    // App-level master switch opts in ALL workspaces.
    if crate::app_settings::load().dns_manage_enabled {
        return true;
    }
    // Otherwise consult the per-workspace flag.
    get_dns_manage_enabled(project_path)
}

/// C1 (0.40.45) — read the PER-WORKSPACE agents-may-create-connections
/// opt-in for `project_path`. Returns `false` (fail-closed) when the
/// project isn't registered or the column reads NULL. Does NOT consider
/// the app-level master flag — use
/// [`agents_can_create_connections_for_path`] for the effective gate.
pub fn get_agents_can_create_connections(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT agents_can_create_connections FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, i64>(0),
    )
    .map(|v| v == 1)
    .unwrap_or(false)
}

/// C1 (0.40.45) — the EFFECTIVE agents-may-create-connections gate for
/// `project_path`.
///
/// An agent may add/remove connections for a workspace iff its
/// per-workspace flag is set OR the app-level `agentsCanCreateConnections`
/// master is on. The app-level flag is a GLOBAL MASTER: enabling it once
/// opts in every workspace, while the default (both OFF) denies,
/// fail-closed.
///
/// This gates ONLY the agent / non-owner path. Owner token and Owner-role
/// (or Admin) connect-users always may mutate connections — the daemon
/// short-circuits privileged actors before consulting this helper.
///
/// Fail-closed: an unknown/unregistered workspace + app-level off → false.
pub fn agents_can_create_connections_for_path(project_path: &str) -> bool {
    if crate::app_settings::load().agents_can_create_connections {
        return true;
    }
    get_agents_can_create_connections(project_path)
}

// ── K2 Mail (prd-email-server-v1 §12) — gating-setting resolvers ────────
//
// Global default in `AppSettings` (`mail_agent_send` / `mail_address_cap`),
// per-workspace override columns on `projects` (migration 0072, NULL =
// inherit). These two resolvers are the ONLY read path later slices may
// use for gating decisions — never read the raw column or the raw
// AppSettings field at a call site.

/// The EFFECTIVE agent-send gating mode for `project_path` (D4):
/// `off` | `approval` | `on`.
///
/// Per-workspace override wins when present AND valid; otherwise the
/// global `AppSettings.mail_agent_send` default. FAIL-CLOSED: an
/// unregistered workspace, a NULL column, or an unrecognized stored
/// value at EITHER level resolves to `off` — outbound mail can never
/// be enabled by accident or by a corrupt value.
pub fn mail_agent_send_for_path(project_path: &str) -> String {
    let per_workspace: Option<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT mail_agent_send FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    };
    let effective = match per_workspace {
        Some(v) => v,
        None => crate::app_settings::load().mail_agent_send,
    };
    if MAIL_AGENT_SEND_MODES.contains(&effective.as_str()) {
        effective
    } else {
        "off".to_string()
    }
}

/// Daemon default concurrent host-session cells per workspace.
///
/// Reads env `K2_SANDBOX_WORKSPACE_CELL_CAP`; falls back to
/// [`DEFAULT_HOST_SESSION_CELL_CAP`] (15) when absent, empty, zero, or
/// unparsable. Aligned with `k2_daemon::sandbox_quota::workspace_cap()`.
pub fn daemon_default_host_session_cell_cap() -> usize {
    match std::env::var("K2_SANDBOX_WORKSPACE_CELL_CAP") {
        Ok(v) => v
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .map(|n| n.min(MAX_HOST_SESSION_CELL_CAP))
            .unwrap_or(DEFAULT_HOST_SESSION_CELL_CAP),
        Err(_) => DEFAULT_HOST_SESSION_CELL_CAP,
    }
}

/// Raw stored `host_session_cell_cap` for `project_path`, if set and
/// positive. `None` means inherit the daemon default (column NULL,
/// invalid, or project unknown). Used by CLI `get` so operators can
/// tell override vs inherit.
pub fn get_host_session_cell_cap_raw(project_path: &str) -> Option<usize> {
    let db = crate::db::shared();
    let conn = db.lock();
    let raw: Option<i64> = conn
        .query_row(
            "SELECT host_session_cell_cap FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten();
    match raw {
        Some(v) if v >= 1 => Some((v as usize).min(MAX_HOST_SESSION_CELL_CAP)),
        _ => None,
    }
}

/// EFFECTIVE concurrent host-session cell cap for `project_path`.
///
/// - Column set to a positive integer → that value, clamped to
///   [`MAX_HOST_SESSION_CELL_CAP`].
/// - Column NULL / invalid / unknown project →
///   [`daemon_default_host_session_cell_cap`] (env or 15).
///
/// Host-session spawn passes this into the sandbox quota acquire path
/// so each workspace can raise its live-cell runway without raising the
/// global env default for every workspace.
pub fn get_host_session_cell_cap(project_path: &str) -> usize {
    get_host_session_cell_cap_raw(project_path)
        .unwrap_or_else(daemon_default_host_session_cell_cap)
}

/// Per-workspace "hide API sessions" — when true, the renderer must not
/// auto-adopt `/v1` host-session / sandbox tabs. Fail-closed: unknown
/// path → false (show sessions).
pub fn get_hide_api_sessions(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT hide_api_sessions FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, i64>(0),
    )
    .ok()
    .is_some_and(|v| v == 1)
}

/// Per-workspace completion chime. Default ON. Unknown path → true
/// (same as a missing column / pre-migration row).
pub fn get_completion_sound_enabled(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT completion_sound_enabled FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, i64>(0),
    )
    .ok()
    .map(|v| v != 0)
    .unwrap_or(true)
}

/// The EFFECTIVE address cap for `project_path` (D6): number of
/// addresses an agent may mint, `0` = unlimited.
///
/// Per-workspace override wins when present and non-negative; otherwise
/// the global `AppSettings.mail_address_cap` default (5). A negative or
/// unreadable stored value falls back to the global default (never to
/// unlimited).
pub fn mail_address_cap_for_path(project_path: &str) -> u32 {
    let per_workspace: Option<i64> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT mail_address_cap FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    };
    match per_workspace {
        Some(v) if v >= 0 => v as u32,
        _ => crate::app_settings::load().mail_address_cap,
    }
}

/// Chunk 2.2 — EFFECTIVE agents-may-manage-Skin-Access gate for
/// `project_path`. Column only: no global master (unlike DNS /
/// connections). Unknown / unregistered path → `false` (fail-closed).
///
/// Owner / Owner-ROLE never consult this helper — the dispatcher
/// short-circuits privileged actors first. Toggle key is the hook
/// principal's workspace UUID resolved to a project path, never a
/// client `project=` / `K2_PROJECT_PATH`.
pub fn agents_can_manage_skin_for_path(project_path: &str) -> bool {
    let db = crate::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT agents_can_manage_skin FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, i64>(0),
    )
    .map(|v| v == 1)
    .unwrap_or(false)
}

/// Persist `projects.agents_can_manage_skin` (0/1). Dedicated writer
/// for `POST /cli/agents-manage-skin` — this field is **not** on
/// [`allowed_project_setting_fields`] (`workspace/set` must 400).
pub fn set_agents_can_manage_skin(project_path: &str, enable: bool) -> Result<(), String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let n = conn
        .execute(
            "UPDATE projects SET agents_can_manage_skin = ?1 WHERE path = ?2",
            rusqlite::params![if enable { 1i64 } else { 0i64 }, project_path],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("Project not found: {project_path}"));
    }
    Ok(())
}

/// EFFECTIVE `db_agent_access` for `project_path`: `off` | `read` | `write`.
///
/// NULL / unknown / unrecognized → `'off'` (fail-closed). Create-only
/// passport: owner tokens always create; agents cannot create unless
/// `'write'`. Interaction (list/dsn/store) uses ownership or sql_grants.
pub fn db_agent_access_for_path(project_path: &str) -> String {
    let per_workspace: Option<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT db_agent_access FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    };
    match per_workspace {
        Some(v) if DB_AGENT_ACCESS_MODES.contains(&v.as_str()) => v,
        _ => "off".to_string(),
    }
}

/// EFFECTIVE ACTIVE-DB cap for `project_path` (D9). 0 = unlimited.
/// NULL / unknown → [`DEFAULT_DB_ACTIVE_CAP`] (1).
pub fn db_active_cap_for_path(project_path: &str) -> u32 {
    let per_workspace: Option<i64> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT db_active_cap FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    };
    match per_workspace {
        Some(v) if v >= 0 => v as u32,
        _ => DEFAULT_DB_ACTIVE_CAP,
    }
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

/// Read the PER-WORKSPACE sandbox FS mode for `project_path` (Sandbox v2, PRD
/// §G2 #1). Fail-SAFE: an unregistered project, a NULL column (the default —
/// existing rows backfill to NULL), a blank value, an unknown string, OR a DB
/// error ALL decode to the PRD LOCKED default [`FsMode::Overlay`] via
/// [`FsMode::from_setting`] — a workspace never silently loses its RO-base
/// overlay guarantee, and a momentary DB hiccup can't downgrade the mode.
pub fn get_workspace_fs_mode(project_path: &str) -> crate::terminal::sandbox::FsMode {
    let db = crate::db::shared();
    let conn = db.lock();
    let raw: Option<String> = conn
        .query_row(
            "SELECT sandbox_fs_mode FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();
    crate::terminal::sandbox::FsMode::from_setting(raw.as_deref())
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

    // ── DNS K1 per-workspace DNS-manage opt-in ─────────────────────

    #[test]
    fn hide_api_sessions_defaults_off_and_round_trips() {
        let path = unique_path("hide-api-sessions");
        let _pid = insert_project(&path);

        let settings = get_project_settings(&path).expect("read default");
        assert_eq!(settings["hideApiSessions"], false);
        assert!(!get_hide_api_sessions(&path));

        update_project_setting(&path, "hide_api_sessions", "1").expect("opt in");
        assert!(get_hide_api_sessions(&path));
        let settings = get_project_settings(&path).expect("read on");
        assert_eq!(settings["hideApiSessions"], true);

        update_project_setting(&path, "hide_api_sessions", "0").expect("opt out");
        assert!(!get_hide_api_sessions(&path));
        let settings = get_project_settings(&path).expect("read off");
        assert_eq!(settings["hideApiSessions"], false);

        let err = update_project_setting(&path, "hide_api_sessions", "true")
            .expect_err("non 0/1 must fail loudly");
        assert!(
            err.contains("hide_api_sessions"),
            "error must name the field, got {err}"
        );
    }

    #[test]
    fn completion_sound_enabled_defaults_on_and_round_trips() {
        let path = unique_path("completion-sound");
        let _pid = insert_project(&path);

        let settings = get_project_settings(&path).expect("read default");
        assert_eq!(settings["completionSoundEnabled"], true);
        assert!(get_completion_sound_enabled(&path));

        update_project_setting(&path, "completion_sound_enabled", "0").expect("mute");
        assert!(!get_completion_sound_enabled(&path));
        let settings = get_project_settings(&path).expect("read off");
        assert_eq!(settings["completionSoundEnabled"], false);

        update_project_setting(&path, "completion_sound_enabled", "1").expect("unmute");
        assert!(get_completion_sound_enabled(&path));
        let settings = get_project_settings(&path).expect("read on");
        assert_eq!(settings["completionSoundEnabled"], true);

        let err = update_project_setting(&path, "completion_sound_enabled", "true")
            .expect_err("non 0/1 must fail loudly");
        assert!(
            err.contains("completion_sound_enabled"),
            "error must name the field, got {err}"
        );
    }

    /// A fresh workspace row defaults to DNS-manage OFF (fail-closed),
    /// surfaces as `dnsManageEnabled: false` in the settings JSON, and
    /// round-trips through `update_project_setting`.
    #[test]
    fn dns_manage_enabled_defaults_off_and_round_trips() {
        let path = unique_path("dns-manage");
        let _pid = insert_project(&path);

        // Fresh row: the security default is OFF.
        let settings = get_project_settings(&path).expect("read default");
        assert_eq!(settings["dnsManageEnabled"], false);
        assert!(!get_dns_manage_enabled(&path));

        // Opt in → reads true.
        update_project_setting(&path, "dns_manage_enabled", "1").expect("opt in");
        assert!(get_dns_manage_enabled(&path));
        let settings = get_project_settings(&path).expect("read on");
        assert_eq!(settings["dnsManageEnabled"], true);

        // Opt back out → reads false.
        update_project_setting(&path, "dns_manage_enabled", "0").expect("opt out");
        assert!(!get_dns_manage_enabled(&path));
    }

    /// A non-'0'/'1' value must be rejected loudly so a typo can't leave
    /// the security gate in an undefined state.
    #[test]
    fn dns_manage_enabled_rejects_bad_value() {
        let path = unique_path("dns-manage-bad");
        let _pid = insert_project(&path);
        let err = update_project_setting(&path, "dns_manage_enabled", "true")
            .expect_err("non 0/1 value must be rejected");
        assert!(
            err.contains("dns_manage_enabled"),
            "error should reference the field, got {err:?}",
        );
    }

    /// An unregistered workspace path fails CLOSED — `false`, never panics.
    #[test]
    fn get_dns_manage_enabled_fails_closed_for_unknown_path() {
        let path = unique_path("dns-manage-missing");
        // No insert — the path doesn't exist in `projects`.
        assert!(!get_dns_manage_enabled(&path));
    }

    /// The EFFECTIVE gate decision (`dns_manage_allowed_for_path`):
    ///   - both flags OFF (default)            → deny (fail-closed)
    ///   - per-workspace ON, app-level OFF     → allow
    ///   - per-workspace OFF, app-level ON     → allow (global master)
    ///   - unknown path + app-level OFF        → deny (fail-closed)
    /// HOME is pointed at a tempdir so the app-level flag in
    /// `~/.k2/settings.json` is isolated; the per-workspace flag lives in
    /// the shared in-memory DB.
    #[test]
    fn dns_manage_effective_or_semantics() {
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        let path = unique_path("dns-manage-effective");
        let _pid = insert_project(&path);

        // Both OFF → deny.
        crate::app_settings::update(serde_json::json!({ "dnsManageEnabled": false }))
            .expect("app off");
        update_project_setting(&path, "dns_manage_enabled", "0").expect("ws off");
        assert!(
            !dns_manage_allowed_for_path(&path),
            "both flags off must deny (fail-closed)",
        );

        // Unknown path + app-level OFF → deny (fail-closed).
        let missing = unique_path("dns-manage-effective-missing");
        assert!(
            !dns_manage_allowed_for_path(&missing),
            "unknown workspace path must deny when master is off",
        );

        // Per-workspace ON, app-level OFF → allow.
        update_project_setting(&path, "dns_manage_enabled", "1").expect("ws on");
        assert!(
            dns_manage_allowed_for_path(&path),
            "per-workspace opt-in must allow even with app-level off",
        );

        // Per-workspace OFF, app-level ON → allow (global master).
        update_project_setting(&path, "dns_manage_enabled", "0").expect("ws off again");
        crate::app_settings::update(serde_json::json!({ "dnsManageEnabled": true }))
            .expect("app on");
        assert!(
            dns_manage_allowed_for_path(&path),
            "app-level master must opt in every workspace",
        );
    }

    // ── C1 per-workspace agents-may-create-connections ─────────────

    /// A fresh workspace row defaults to agents-create-connections OFF
    /// (fail-closed), surfaces as `agentsCanCreateConnections: false`
    /// in the settings JSON, and round-trips through
    /// `update_project_setting`.
    #[test]
    fn agents_can_create_connections_defaults_off_and_round_trips() {
        let path = unique_path("agents-conn");
        let _pid = insert_project(&path);

        let settings = get_project_settings(&path).expect("read default");
        assert_eq!(settings["agentsCanCreateConnections"], false);
        assert!(!get_agents_can_create_connections(&path));

        update_project_setting(&path, "agents_can_create_connections", "1").expect("opt in");
        assert!(get_agents_can_create_connections(&path));
        let settings = get_project_settings(&path).expect("read on");
        assert_eq!(settings["agentsCanCreateConnections"], true);

        update_project_setting(&path, "agents_can_create_connections", "0").expect("opt out");
        assert!(!get_agents_can_create_connections(&path));
    }

    #[test]
    fn agents_can_create_connections_rejects_bad_value() {
        let path = unique_path("agents-conn-bad");
        let _pid = insert_project(&path);
        let err = update_project_setting(&path, "agents_can_create_connections", "true")
            .expect_err("non 0/1 value must be rejected");
        assert!(
            err.contains("agents_can_create_connections"),
            "error should reference the field, got {err:?}",
        );
    }

    #[test]
    fn get_agents_can_create_connections_fails_closed_for_unknown_path() {
        let path = unique_path("agents-conn-missing");
        assert!(!get_agents_can_create_connections(&path));
    }

    // ── Phase 0b per-workspace API guest policy ─────────────────────

    /// Fresh row / empty → platform default; custom text round-trips;
    /// settings JSON exposes the effective policy.
    #[test]
    fn api_guest_policy_defaults_and_round_trips() {
        let path = unique_path("api-guest");
        let _pid = insert_project(&path);

        assert_eq!(get_api_guest_policy(&path), DEFAULT_API_GUEST_POLICY);
        assert_eq!(get_api_guest_policy_raw(&path), None);
        let settings = get_project_settings(&path).expect("read default");
        assert_eq!(
            settings["apiGuestPolicy"].as_str().expect("string"),
            DEFAULT_API_GUEST_POLICY,
        );

        let custom = "Custom guest: read-only wiki Q&A only.";
        update_project_setting(&path, "api_guest_policy", custom).expect("set");
        assert_eq!(get_api_guest_policy(&path), custom);
        assert_eq!(get_api_guest_policy_raw(&path).as_deref(), Some(custom));
        let settings = get_project_settings(&path).expect("read custom");
        assert_eq!(settings["apiGuestPolicy"], custom);

        update_project_setting(&path, "api_guest_policy", "").expect("clear");
        assert_eq!(get_api_guest_policy(&path), DEFAULT_API_GUEST_POLICY);
        assert_eq!(get_api_guest_policy_raw(&path), None);
    }

    #[test]
    fn api_guest_policy_unknown_path_uses_default() {
        let path = unique_path("api-guest-missing");
        assert_eq!(get_api_guest_policy(&path), DEFAULT_API_GUEST_POLICY);
        assert_eq!(get_api_guest_policy_raw(&path), None);
    }

    // ── Phase 1 per-workspace public wiki chat ──────────────────────

    /// Fresh row defaults OFF; 0/1 round-trips; settings JSON exposes
    /// `wikiPublicChat`; unknown path fails closed.
    #[test]
    fn wiki_public_chat_defaults_off_and_round_trips() {
        let path = unique_path("wiki-public-chat");
        let _pid = insert_project(&path);

        assert!(
            !get_wiki_public_chat(&path),
            "wiki_public_chat must default OFF (D6)",
        );
        let settings = get_project_settings(&path).expect("read default");
        assert_eq!(settings["wikiPublicChat"], false);

        update_project_setting(&path, "wiki_public_chat", "1").expect("opt in");
        assert!(get_wiki_public_chat(&path));
        let settings = get_project_settings(&path).expect("read on");
        assert_eq!(settings["wikiPublicChat"], true);

        update_project_setting(&path, "wiki_public_chat", "0").expect("opt out");
        assert!(!get_wiki_public_chat(&path));
        let settings = get_project_settings(&path).expect("read off");
        assert_eq!(settings["wikiPublicChat"], false);
    }

    #[test]
    fn wiki_public_chat_rejects_bad_value() {
        let path = unique_path("wiki-public-chat-bad");
        let _pid = insert_project(&path);
        let err = update_project_setting(&path, "wiki_public_chat", "true")
            .expect_err("non 0/1 value must be rejected");
        assert!(
            err.contains("wiki_public_chat"),
            "error should reference the field, got {err:?}",
        );
    }

    #[test]
    fn get_wiki_public_chat_fails_closed_for_unknown_path() {
        let path = unique_path("wiki-public-chat-missing");
        assert!(!get_wiki_public_chat(&path));
    }

    /// Effective gate OR semantics (same as dns_manage / remote_instruct):
    ///   - both OFF → deny
    ///   - per-workspace ON, app OFF → allow
    ///   - per-workspace OFF, app ON → allow
    ///   - unknown path + app OFF → deny
    #[test]
    fn agents_can_create_connections_effective_or_semantics() {
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        let path = unique_path("agents-conn-effective");
        let _pid = insert_project(&path);

        crate::app_settings::update(serde_json::json!({ "agentsCanCreateConnections": false }))
            .expect("app off");
        update_project_setting(&path, "agents_can_create_connections", "0").expect("ws off");
        assert!(
            !agents_can_create_connections_for_path(&path),
            "both flags off must deny (fail-closed)",
        );

        let missing = unique_path("agents-conn-effective-missing");
        assert!(
            !agents_can_create_connections_for_path(&missing),
            "unknown workspace path must deny when master is off",
        );

        update_project_setting(&path, "agents_can_create_connections", "1").expect("ws on");
        assert!(
            agents_can_create_connections_for_path(&path),
            "per-workspace opt-in must allow even with app-level off",
        );

        update_project_setting(&path, "agents_can_create_connections", "0").expect("ws off again");
        crate::app_settings::update(serde_json::json!({ "agentsCanCreateConnections": true }))
            .expect("app on");
        assert!(
            agents_can_create_connections_for_path(&path),
            "app-level master must opt in every workspace",
        );
    }

    // ── K2 Mail gating settings (prd-email-server-v1 §12) ──────────

    /// `mail_agent_send`: write-validated enum; the EFFECTIVE resolver
    /// prefers the per-workspace override, inherits the global default
    /// when NULL, and fail-closes to "off" everywhere else.
    #[test]
    fn mail_agent_send_defaults_off_and_override_wins() {
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        let path = unique_path("mail-send");
        let _pid = insert_project(&path);

        // Everything default → off (fail-closed), including for a path
        // that isn't registered at all.
        assert_eq!(mail_agent_send_for_path(&path), "off");
        assert_eq!(mail_agent_send_for_path("/tmp/never-registered-mail"), "off");

        // Global default flips → un-overridden workspaces inherit it.
        crate::app_settings::update(serde_json::json!({ "mailAgentSend": "approval" }))
            .expect("set global");
        assert_eq!(mail_agent_send_for_path(&path), "approval");

        // Per-workspace override wins over the global.
        update_project_setting(&path, "mail_agent_send", "on").expect("override on");
        assert_eq!(mail_agent_send_for_path(&path), "on");
        update_project_setting(&path, "mail_agent_send", "off").expect("override off");
        assert_eq!(mail_agent_send_for_path(&path), "off");

        // A corrupt GLOBAL value fails closed to off (the workspace
        // override is cleared by writing garbage directly — the write
        // path would reject it, which is the next test).
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE projects SET mail_agent_send = NULL WHERE path = ?1",
                rusqlite::params![path],
            )
            .expect("clear override");
        }
        crate::app_settings::update(serde_json::json!({ "mailAgentSend": "yolo" }))
            .expect("set corrupt global");
        assert_eq!(
            mail_agent_send_for_path(&path),
            "off",
            "unknown global mode must fail closed"
        );

        // A corrupt STORED override also fails closed.
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE projects SET mail_agent_send = 'always' WHERE path = ?1",
                rusqlite::params![path],
            )
            .expect("corrupt override directly");
        }
        assert_eq!(
            mail_agent_send_for_path(&path),
            "off",
            "unknown stored override must fail closed"
        );
    }

    /// The write path rejects anything outside off|approval|on and any
    /// non-integer cap, loudly.
    #[test]
    fn mail_settings_write_validation() {
        let path = unique_path("mail-validate");
        let _pid = insert_project(&path);

        let err = update_project_setting(&path, "mail_agent_send", "always")
            .expect_err("bad mode must be rejected");
        assert!(err.contains("mail_agent_send"), "got {err:?}");

        for bad in ["-1", "five", "1.5", ""] {
            let err = update_project_setting(&path, "mail_address_cap", bad)
                .expect_err("bad cap must be rejected");
            assert!(err.contains("mail_address_cap"), "'{bad}' → {err:?}");
        }
    }

    /// Chunk 2.2 — column-only Skin Access passport. Default OFF;
    /// unknown path false; not writable via `workspace/set`.
    #[test]
    fn agents_can_manage_skin_defaults_off_unknown_false_and_round_trips() {
        let path = unique_path("agents-manage-skin");
        let _pid = insert_project(&path);

        assert!(
            !agents_can_manage_skin_for_path(&path),
            "fresh row must default OFF"
        );
        assert!(
            !agents_can_manage_skin_for_path("/tmp/never-registered-skin-manage"),
            "unknown path must fail closed"
        );
        assert!(
            !allowed_project_setting_fields().contains(&"agents_can_manage_skin"),
            "must not be on workspace/set allowlist"
        );

        set_agents_can_manage_skin(&path, true).expect("opt in");
        assert!(agents_can_manage_skin_for_path(&path));
        set_agents_can_manage_skin(&path, false).expect("opt out");
        assert!(!agents_can_manage_skin_for_path(&path));

        let err = update_project_setting(&path, "agents_can_manage_skin", "1")
            .expect_err("workspace/set must reject the column");
        assert!(
            err.contains("Unknown setting"),
            "got {err:?}"
        );
        assert!(
            !agents_can_manage_skin_for_path(&path),
            "rejected workspace/set must not flip the column"
        );

        let missing = unique_path("agents-manage-skin-missing");
        let err = set_agents_can_manage_skin(&missing, true)
            .expect_err("unknown path write must fail");
        assert!(err.contains("Project not found"), "got {err:?}");
    }

    /// Data sidecar passport: NULL/unknown → off; write-validated
    /// off/read/write; cap NULL → 1, 0 = unlimited.
    #[test]
    fn db_agent_access_and_cap_defaults_and_validation() {
        let path = unique_path("db-access");
        let _pid = insert_project(&path);

        assert_eq!(db_agent_access_for_path(&path), "off");
        assert_eq!(db_agent_access_for_path("/tmp/never-registered-db"), "off");
        assert_eq!(db_active_cap_for_path(&path), DEFAULT_DB_ACTIVE_CAP);
        assert_eq!(db_active_cap_for_path("/tmp/never-registered-db"), 1);

        update_project_setting(&path, "db_agent_access", "read").expect("read");
        assert_eq!(db_agent_access_for_path(&path), "read");
        update_project_setting(&path, "db_agent_access", "write").expect("write");
        assert_eq!(db_agent_access_for_path(&path), "write");
        update_project_setting(&path, "db_agent_access", "off").expect("off");
        assert_eq!(db_agent_access_for_path(&path), "off");

        let err = update_project_setting(&path, "db_agent_access", "admin")
            .expect_err("bad mode");
        assert!(err.contains("db_agent_access"), "got {err:?}");

        update_project_setting(&path, "db_active_cap", "0").expect("unlimited");
        assert_eq!(db_active_cap_for_path(&path), 0);
        update_project_setting(&path, "db_active_cap", "3").expect("3");
        assert_eq!(db_active_cap_for_path(&path), 3);
        let err = update_project_setting(&path, "db_active_cap", "-1").expect_err("neg");
        assert!(err.contains("db_active_cap"), "got {err:?}");
    }

    // ── Per-workspace host-session concurrent cell cap ──────────────

    /// Fresh row inherits daemon default (15); set 64 round-trips;
    /// clear via "default" returns to inherit; settings JSON exposes
    /// null when inheriting and the number when set.
    #[test]
    fn host_session_cell_cap_defaults_and_round_trips() {
        let path = unique_path("hs-cell-cap");
        let _pid = insert_project(&path);

        // Fresh / unknown → daemon default (env unset in tests → 15).
        assert_eq!(get_host_session_cell_cap_raw(&path), None);
        assert_eq!(
            get_host_session_cell_cap(&path),
            DEFAULT_HOST_SESSION_CELL_CAP
        );
        assert_eq!(
            get_host_session_cell_cap("/tmp/never-registered-hs-cap"),
            DEFAULT_HOST_SESSION_CELL_CAP
        );
        let settings = get_project_settings(&path).expect("read default");
        assert!(
            settings["hostSessionCellCap"].is_null(),
            "inherit must surface as JSON null, got {:?}",
            settings["hostSessionCellCap"]
        );

        update_project_setting(&path, "host_session_cell_cap", "64").expect("set 64");
        assert_eq!(get_host_session_cell_cap_raw(&path), Some(64));
        assert_eq!(get_host_session_cell_cap(&path), 64);
        let settings = get_project_settings(&path).expect("read set");
        assert_eq!(settings["hostSessionCellCap"], 64);

        // Clear via "default" → inherit again.
        update_project_setting(&path, "host_session_cell_cap", "default").expect("clear");
        assert_eq!(get_host_session_cell_cap_raw(&path), None);
        assert_eq!(
            get_host_session_cell_cap(&path),
            DEFAULT_HOST_SESSION_CELL_CAP
        );
        let settings = get_project_settings(&path).expect("read cleared");
        assert!(settings["hostSessionCellCap"].is_null());

        // Empty and "null" also clear.
        update_project_setting(&path, "host_session_cell_cap", "32").expect("set 32");
        update_project_setting(&path, "host_session_cell_cap", "").expect("clear empty");
        assert_eq!(get_host_session_cell_cap_raw(&path), None);
        update_project_setting(&path, "host_session_cell_cap", "16").expect("set 16");
        update_project_setting(&path, "host_session_cell_cap", "null").expect("clear null");
        assert_eq!(get_host_session_cell_cap_raw(&path), None);
    }

    /// Reject 0, non-numeric, and values above the daemon ceiling (512).
    #[test]
    fn host_session_cell_cap_write_validation() {
        let path = unique_path("hs-cell-cap-val");
        let _pid = insert_project(&path);

        for bad in ["0", "-1", "five", "1.5", "1000000", "513"] {
            let err = update_project_setting(&path, "host_session_cell_cap", bad)
                .expect_err("bad cap must be rejected");
            assert!(
                err.contains("host_session_cell_cap"),
                "'{bad}' → {err:?}"
            );
        }

        // Boundary: 1 and MAX accepted.
        update_project_setting(&path, "host_session_cell_cap", "1").expect("min 1");
        assert_eq!(get_host_session_cell_cap(&path), 1);
        update_project_setting(
            &path,
            "host_session_cell_cap",
            &MAX_HOST_SESSION_CELL_CAP.to_string(),
        )
        .expect("max 512");
        assert_eq!(
            get_host_session_cell_cap(&path),
            MAX_HOST_SESSION_CELL_CAP
        );
        // 64 is the product runway example.
        update_project_setting(&path, "host_session_cell_cap", "64").expect("accept 64");
        assert_eq!(get_host_session_cell_cap(&path), 64);
    }

    /// A smuggled over-ceiling value is clamped on read; zero/negative
    /// falls through to the daemon default.
    #[test]
    fn host_session_cell_cap_read_clamps_corrupt() {
        let path = unique_path("hs-cell-cap-clamp");
        let _pid = insert_project(&path);

        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE projects SET host_session_cell_cap = 9999 WHERE path = ?1",
                rusqlite::params![path],
            )
            .expect("smuggle high");
        }
        assert_eq!(
            get_host_session_cell_cap(&path),
            MAX_HOST_SESSION_CELL_CAP,
            "over-ceiling stored value must clamp"
        );

        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE projects SET host_session_cell_cap = 0 WHERE path = ?1",
                rusqlite::params![path],
            )
            .expect("smuggle zero");
        }
        assert_eq!(
            get_host_session_cell_cap(&path),
            DEFAULT_HOST_SESSION_CELL_CAP,
            "zero stored value must inherit default"
        );
    }

    /// `mail_address_cap`: default 5 (global), per-workspace override
    /// wins, 0 = unlimited passes validation.
    #[test]
    fn mail_address_cap_default_and_override() {
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        let path = unique_path("mail-cap");
        let _pid = insert_project(&path);

        // PRD default: 5 — for registered AND unregistered paths.
        assert_eq!(mail_address_cap_for_path(&path), 5);
        assert_eq!(mail_address_cap_for_path("/tmp/never-registered-cap"), 5);

        // Global default is owner-tunable.
        crate::app_settings::update(serde_json::json!({ "mailAddressCap": 9 }))
            .expect("set global cap");
        assert_eq!(mail_address_cap_for_path(&path), 9);

        // Per-workspace override wins; 0 = unlimited is storable.
        update_project_setting(&path, "mail_address_cap", "2").expect("override 2");
        assert_eq!(mail_address_cap_for_path(&path), 2);
        update_project_setting(&path, "mail_address_cap", "0").expect("override 0");
        assert_eq!(mail_address_cap_for_path(&path), 0, "0 = unlimited");

        // A negative value smuggled in directly falls back to the
        // global default (never to unlimited).
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "UPDATE projects SET mail_address_cap = -3 WHERE path = ?1",
                rusqlite::params![path],
            )
            .expect("corrupt cap directly");
        }
        assert_eq!(mail_address_cap_for_path(&path), 9, "negative → global default");
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
    fn agentic_enabled_is_always_on() {
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // GA: accessor always returns true regardless of disk state.
        assert!(
            get_agentic_enabled(),
            "fresh install → agentic systems are always on",
        );

        set_agentic_enabled(true).expect("set true");
        assert!(get_agentic_enabled(), "after set(true), still on");

        // Off is a no-op for the public API — still reports on, and
        // force-writes true so stale false cannot stick.
        set_agentic_enabled(false).expect("set false must not error");
        assert!(
            get_agentic_enabled(),
            "after set(false), public API still reports on",
        );
        assert!(
            crate::app_settings::load().agentic_systems_enabled,
            "disk must store true after any set_agentic_enabled call",
        );
    }

    #[test]
    fn keep_daemon_on_quit_round_trips_through_app_settings() {
        let _g = HOME_TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Default when the JSON file is absent is `true` — see the doc
        // comment on `get_keep_daemon_on_quit` for the rationale.
        assert!(
            get_keep_daemon_on_quit(),
            "fresh ~/.k2/settings.json → keep_daemon_on_quit defaults to true",
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

    #[test]
    fn default_model_empty_clears_to_null_and_force_validates() {
        let path = unique_path("default-model");
        let _pid = insert_project(&path);

        update_project_setting(&path, "default_model", "opus").expect("set model");
        let settings = get_project_settings(&path).expect("read");
        assert_eq!(settings["defaultModel"], "opus");
        assert_eq!(settings["forceModelOnResume"], false);

        update_project_setting(&path, "force_model_on_resume", "1").expect("set force");
        let settings = get_project_settings(&path).expect("read force");
        assert_eq!(settings["forceModelOnResume"], true);

        update_project_setting(&path, "default_model", "").expect("clear model");
        let settings = get_project_settings(&path).expect("read cleared");
        assert!(
            settings["defaultModel"].is_null(),
            "empty default_model must store NULL; got {}",
            settings["defaultModel"]
        );

        let err = update_project_setting(&path, "force_model_on_resume", "yes")
            .expect_err("non 0/1 must be rejected");
        assert!(
            err.contains("force_model_on_resume"),
            "error should name the field, got {err:?}"
        );
    }
}
