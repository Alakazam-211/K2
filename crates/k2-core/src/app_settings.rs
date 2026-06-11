//! Daemon-owned global application settings (`~/.k2so/settings.json`).
//!
//! Phase 2 Unit 7a — moves the `AppSettings` schema + on-disk
//! tmp-and-rename writer from `src-tauri/src/commands/settings.rs` into
//! k2so-core so the daemon can serve `/cli/settings/{get,update,reset}`
//! without Tauri being present.
//!
//! # Two-writers race (F3) — resolved here
//!
//! Pre-Unit-7a, both Tauri (`settings_update`) and the daemon
//! (`companion_host::persist_companion_password_fields`) wrote to the
//! same `~/.k2so/settings.json`. tmp+rename meant readers never saw a
//! torn file, but a Tauri-side rotate of `companion.username` was a
//! no-op on the daemon's in-memory companion runtime (Tauri's `STATE`
//! is empty post-Unit-1). The legacy live tokens stayed valid until
//! their TTL expired.
//!
//! After Unit 7a, Tauri's `settings_update` proxies to
//! `POST /cli/settings/update`. The daemon takes the global
//! [`SETTINGS_LOCK`] before load+merge+save, then — after the file is
//! durable — compares old-vs-new companion-affecting fields and calls
//! `k2_core::companion::invalidate_all_sessions` server-side, where
//! the live `STATE` actually lives. The race window collapses to zero:
//! a single critical section owns the merge, and the post-write
//! invalidation is in-process with the sessions it's killing.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::log_debug;

// ── Settings types (moved verbatim from src-tauri/src/commands/settings.rs) ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSettings {
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    #[serde(default = "default_cursor_style")]
    pub cursor_style: String,
    #[serde(default = "default_scrollback")]
    pub scrollback: u32,
    #[serde(default = "default_true")]
    pub natural_text_editing: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        default_terminal()
    }
}

fn default_font_family() -> String {
    "MesloLGM Nerd Font".to_string()
}
fn default_font_size() -> u32 {
    13
}
fn default_cursor_style() -> String {
    "bar".to_string()
}
fn default_scrollback() -> u32 {
    5000
}

fn default_agent() -> String {
    "claude".to_string()
}

/// P1.C — default Active-Bar tenure window: 24 hours.
fn default_active_window_hours() -> u32 {
    24
}

fn default_left_panel_tab() -> String {
    "files".to_string()
}
fn default_right_panel_tab() -> String {
    "history".to_string()
}
fn default_left_panel_tabs() -> Vec<String> {
    vec!["files".to_string(), "agents".to_string()]
}
fn default_right_panel_tabs() -> Vec<String> {
    vec![
        "history".to_string(),
        "changes".to_string(),
        "reviews".to_string(),
    ]
}

fn default_terminal() -> TerminalSettings {
    TerminalSettings {
        font_family: "MesloLGM Nerd Font".to_string(),
        font_size: 13,
        cursor_style: "bar".to_string(),
        scrollback: 5000,
        natural_text_editing: true,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TimerSettings {
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default = "default_true")]
    pub countdown_enabled: bool,
    #[serde(default = "default_countdown_theme")]
    pub countdown_theme: String,
    #[serde(default)]
    pub skip_memo: bool,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub custom_themes: Vec<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

fn default_countdown_theme() -> String {
    "rocket".to_string()
}

impl Default for TimerSettings {
    fn default() -> Self {
        Self {
            visible: true,
            countdown_enabled: true,
            countdown_theme: "rocket".to_string(),
            skip_memo: false,
            timezone: String::new(),
            custom_themes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default = "default_terminal")]
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub keybindings: HashMap<String, String>,
    #[serde(default, alias = "projects")]
    pub project_settings: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub focus_groups_enabled: bool,
    #[serde(default)]
    pub active_focus_group_id: Option<String>,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    #[serde(default)]
    pub left_panel_open: bool,
    #[serde(default)]
    pub right_panel_open: bool,
    #[serde(default = "default_left_panel_tab")]
    pub left_panel_active_tab: String,
    #[serde(default = "default_right_panel_tab")]
    pub right_panel_active_tab: String,
    #[serde(default = "default_left_panel_tabs")]
    pub left_panel_tabs: Vec<String>,
    #[serde(default = "default_right_panel_tabs")]
    pub right_panel_tabs: Vec<String>,
    /// Deprecated: workspace layouts now stored in SQLite workspace_layouts table.
    /// Kept for deserialization compat with old settings.json files; skipped on write.
    #[serde(default, skip_serializing)]
    pub workspace_layouts: HashMap<String, serde_json::Value>,
    #[serde(default = "default_agent")]
    pub default_agent: String,
    #[serde(default)]
    pub ai_assistant_enabled: bool,
    #[serde(default)]
    pub timer: TimerSettings,
    #[serde(default)]
    pub agentic_systems_enabled: bool,
    #[serde(default = "default_true")]
    pub keep_daemon_on_quit: bool,
    #[serde(default)]
    pub claude_auth_auto_refresh: bool,
    #[serde(default)]
    pub last_active_project_id: Option<String>,
    #[serde(default)]
    pub last_active_workspace_id: Option<String>,
    /// P1.C — how many hours a workspace stays in the Active Bar after the
    /// user last interacted with it (Active-Bar rule 2). Persisted here so
    /// both `ActiveBar` and a future reaper read one source of truth.
    /// A typed field is required: `AppSettings` round-trips through
    /// `serde_json::from_value` on every `load`/`update`, which silently
    /// drops keys with no matching field — an untyped key would never
    /// persist. Defaults to 24 for old settings.json snapshots.
    #[serde(default = "default_active_window_hours")]
    pub active_window_hours: u32,
    #[serde(default)]
    pub editor: EditorSettings,
    #[serde(default)]
    pub companion: CompanionSettings,
    /// Heartbeat wake scheduler — controls whether/how often launchd
    /// wakes the laptop to fire agent heartbeats.
    #[serde(default)]
    pub wake_scheduler: WakeSchedulerSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WakeSchedulerSettings {
    #[serde(default = "default_wake_mode")]
    pub mode: String,
    #[serde(default = "default_wake_interval")]
    pub interval_minutes: u32,
    /// `WakeSystem: true` in the plist — lets launchd wake the laptop
    /// from sleep to fire the heartbeat.
    #[serde(default)]
    pub wake_system: bool,
}

fn default_wake_mode() -> String {
    "on_demand".to_string()
}
fn default_wake_interval() -> u32 {
    1
}

impl Default for WakeSchedulerSettings {
    fn default() -> Self {
        Self {
            mode: default_wake_mode(),
            interval_minutes: default_wake_interval(),
            wake_system: false,
        }
    }
}

/// Mobile Companion API settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub username: String,
    /// Legacy on-disk copy of the argon2 password hash. Post-0.32.12 the
    /// canonical hash lives in the macOS Keychain; this field is cleared
    /// after migration.
    #[serde(default)]
    pub password_hash: String,
    /// True once a companion password has been configured.
    #[serde(default)]
    pub password_set: bool,
    #[serde(default)]
    pub ngrok_auth_token: String,
    #[serde(default)]
    pub ngrok_domain: String,
    /// Allowlist of browser origins permitted to use the companion API.
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// When false (default), the companion API refuses
    /// `/companion/terminal/spawn` and `/companion/terminal/spawn-background`.
    #[serde(default)]
    pub allow_remote_spawn: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditorSettings {
    #[serde(default = "default_tab_size")]
    pub tab_size: u32,
    #[serde(default)]
    pub word_wrap: bool,
    #[serde(default)]
    pub show_whitespace: bool,
    #[serde(default = "default_editor_font_size")]
    pub font_size: u32,
    #[serde(default = "default_true")]
    pub indent_guides: bool,
    #[serde(default = "default_true")]
    pub fold_gutter: bool,
    #[serde(default = "default_true")]
    pub autocomplete: bool,
    #[serde(default = "default_true")]
    pub bracket_matching: bool,
    #[serde(default = "default_true")]
    pub line_numbers: bool,
    #[serde(default = "default_true")]
    pub highlight_active_line: bool,
    #[serde(default)]
    pub sticky_scroll: bool,
    #[serde(default)]
    pub minimap: bool,
    #[serde(default = "default_editor_theme")]
    pub theme: String,
    #[serde(default = "default_editor_font_family")]
    pub font_family: String,
    #[serde(default)]
    pub font_ligatures: bool,
    #[serde(default = "default_cursor_style")]
    pub cursor_style: String,
    #[serde(default = "default_true")]
    pub cursor_blink: bool,
    #[serde(default)]
    pub scroll_past_end: bool,
    #[serde(default = "default_true")]
    pub scrollbar_annotations: bool,
    #[serde(default = "default_diff_style")]
    pub diff_style: String,
    #[serde(default)]
    pub format_on_save: bool,
    #[serde(default)]
    pub vim_mode: bool,
}

fn default_tab_size() -> u32 {
    2
}
fn default_editor_font_size() -> u32 {
    12
}
fn default_diff_style() -> String {
    "gutter".to_string()
}
fn default_editor_theme() -> String {
    "k2so-dark".to_string()
}
fn default_editor_font_family() -> String {
    "MesloLGM Nerd Font".to_string()
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: 2,
            word_wrap: false,
            show_whitespace: false,
            font_size: 12,
            indent_guides: true,
            fold_gutter: true,
            autocomplete: true,
            bracket_matching: true,
            line_numbers: true,
            highlight_active_line: true,
            sticky_scroll: false,
            minimap: false,
            theme: "k2so-dark".to_string(),
            font_family: "MesloLGM Nerd Font".to_string(),
            font_ligatures: false,
            cursor_style: "bar".to_string(),
            cursor_blink: true,
            scroll_past_end: false,
            scrollbar_annotations: true,
            diff_style: "gutter".to_string(),
            format_on_save: false,
            vim_mode: false,
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            terminal: default_terminal(),
            keybindings: HashMap::new(),
            project_settings: HashMap::new(),
            focus_groups_enabled: false,
            active_focus_group_id: None,
            sidebar_collapsed: false,
            left_panel_open: false,
            right_panel_open: false,
            left_panel_active_tab: default_left_panel_tab(),
            right_panel_active_tab: default_right_panel_tab(),
            left_panel_tabs: default_left_panel_tabs(),
            right_panel_tabs: default_right_panel_tabs(),
            workspace_layouts: HashMap::new(),
            default_agent: "claude".to_string(),
            ai_assistant_enabled: false,
            timer: TimerSettings::default(),
            agentic_systems_enabled: false,
            keep_daemon_on_quit: true,
            claude_auth_auto_refresh: false,
            last_active_project_id: None,
            last_active_workspace_id: None,
            active_window_hours: default_active_window_hours(),
            editor: EditorSettings::default(),
            companion: CompanionSettings::default(),
            wake_scheduler: WakeSchedulerSettings::default(),
        }
    }
}

// ── File I/O ─────────────────────────────────────────────────────────────

fn settings_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| {
            log_debug!("[app_settings] WARNING: no home dir, using current dir");
            PathBuf::from(".")
        })
        .join(".k2")
}

fn settings_file() -> PathBuf {
    settings_dir().join("settings.json")
}

fn ensure_dir() {
    let dir = settings_dir();
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
}

/// Recursive deep-merge of `overlay` into `base`. Matches the behavior
/// of the legacy Tauri `deep_merge` so old settings.json files with
/// partial overlays continue to round-trip identically.
fn deep_merge(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    if let (Some(base_map), Some(overlay_map)) = (base.as_object_mut(), overlay.as_object()) {
        for (key, val) in overlay_map {
            if val.is_object() && base_map.get(key).map_or(false, |v| v.is_object()) {
                let mut base_val = base_map.get(key).unwrap().clone();
                deep_merge(&mut base_val, val);
                base_map.insert(key.clone(), base_val);
            } else {
                base_map.insert(key.clone(), val.clone());
            }
        }
    }
}

#[cfg(unix)]
fn restrict_mode(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(file, fs::Permissions::from_mode(0o600)) {
        log_debug!(
            "[app_settings] WARN: chmod 0o600 on {}: {}",
            file.display(),
            e
        );
    }
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) {}

#[cfg(unix)]
fn restrict_mode_if_wider(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(file) {
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            if let Err(e) = fs::set_permissions(file, fs::Permissions::from_mode(0o600)) {
                log_debug!(
                    "[app_settings] WARN: tighten mode on {}: {}",
                    file.display(),
                    e
                );
            }
        }
    }
}

#[cfg(not(unix))]
fn restrict_mode_if_wider(_file: &Path) {}

/// Process-wide lock around `load + merge + save` critical sections.
/// The on-disk file uses tmp+rename so readers never see a torn write,
/// but the lock makes the merge atomic across concurrent
/// `update(partial)` calls inside one process. Sufficient because the
/// daemon is the sole writer now (the Tauri-side proxy hits the daemon
/// over HTTP, so it runs through this same lock).
static SETTINGS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn settings_lock() -> &'static Mutex<()> {
    SETTINGS_LOCK.get_or_init(|| Mutex::new(()))
}

/// Read `~/.k2so/settings.json` and parse into `AppSettings`. Missing
/// or malformed files yield `AppSettings::default()`. Always
/// deep-merges with defaults so freshly-added keys are present in
/// returned values.
///
/// Legacy "projects" key is migrated in-memory to "projectSettings"
/// matching the pre-Unit-7a Tauri reader.
pub fn load() -> AppSettings {
    ensure_dir();
    let file = settings_file();
    if !file.exists() {
        return AppSettings::default();
    }
    restrict_mode_if_wider(&file);
    let raw = match fs::read_to_string(&file) {
        Ok(s) => s,
        Err(_) => return AppSettings::default(),
    };
    let mut parsed: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return AppSettings::default(),
    };
    if let Some(obj) = parsed.as_object_mut() {
        if obj.contains_key("projects") && !obj.contains_key("projectSettings") {
            if let Some(v) = obj.remove("projects") {
                obj.insert("projectSettings".to_string(), v);
            }
        }
    }
    let mut defaults = serde_json::to_value(AppSettings::default()).unwrap_or_default();
    deep_merge(&mut defaults, &parsed);
    serde_json::from_value(defaults).unwrap_or_default()
}

/// Write `settings` to `~/.k2so/settings.json` via tmp+rename so a
/// concurrent reader never sees a partial file. Restricts the result
/// to mode 0o600 because the payload includes the legacy password
/// hash and the ngrok auth token.
pub fn save(settings: &AppSettings) -> Result<(), String> {
    ensure_dir();
    let file = settings_file();
    let tmp = file.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("serialize settings: {e}"))?;
    fs::write(&tmp, &json).map_err(|e| format!("write {tmp:?}: {e}"))?;
    fs::rename(&tmp, &file).map_err(|e| format!("rename {tmp:?} -> {file:?}: {e}"))?;
    restrict_mode(&file);
    Ok(())
}

/// Atomic load + deep-merge `partial` + save. Returns the post-merge
/// `AppSettings` so callers can echo it back to the renderer in one
/// round-trip.
///
/// **F3 closure:** if companion-affecting fields (`companion.username`,
/// `companion.passwordHash`, derived credentials) change between old
/// and new, calls
/// [`crate::companion::invalidate_all_sessions`] **after** the write
/// completes. The invalidation runs in the same process as the live
/// companion runtime — no two-writers race, no stale TTL window. The
/// emit is best-effort: a missing companion runtime (e.g. during
/// daemon boot before the autostart hook has fired) is a no-op inside
/// `invalidate_all_sessions`, which is exactly the correct behavior.
pub fn update(partial: serde_json::Value) -> Result<AppSettings, String> {
    let _guard = settings_lock().lock();
    let current = load();
    let mut current_val = serde_json::to_value(&current)
        .map_err(|e| format!("serialize current settings: {e}"))?;
    deep_merge(&mut current_val, &partial);
    let merged: AppSettings = serde_json::from_value(current_val)
        .map_err(|e| format!("deserialize merged settings: {e}"))?;

    let creds_changed = current.companion.password_hash != merged.companion.password_hash
        || current.companion.username != merged.companion.username;

    save(&merged)?;

    if creds_changed {
        crate::companion::invalidate_all_sessions("credentials changed");
    }

    Ok(merged)
}

/// Restore defaults: write `AppSettings::default()` over the on-disk
/// file, clear the macOS-Keychain password hash (so it can't outlive
/// a reset and re-activate later), and invalidate every live companion
/// session.
pub fn reset() -> Result<AppSettings, String> {
    let _guard = settings_lock().lock();
    let defaults = AppSettings::default();
    save(&defaults)?;

    // Best-effort Keychain delete. The helper swallows its own errors
    // (non-macOS dev / no `security` CLI) — a missing Keychain entry
    // also won't fail the reset, which matches the legacy Tauri
    // behavior.
    crate::companion::keychain::delete_password_hash();
    crate::companion::invalidate_all_sessions("settings reset");

    Ok(defaults)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test points HOME at a fresh tempdir so `~/.k2so/settings.json`
    /// is isolated from the developer's real settings file and from
    /// concurrent tests.
    struct HomeGuard {
        original: Option<std::ffi::OsString>,
        _tmp: tempdir_lite::TempDir,
    }

    impl HomeGuard {
        fn new() -> Self {
            let tmp = tempdir_lite::TempDir::new("k2so-app-settings-test");
            let original = std::env::var_os("HOME");
            std::env::set_var("HOME", tmp.path());
            Self {
                original,
                _tmp: tmp,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    // Re-use the crate-wide HOME_LOCK from `themes` so the HOME-mutating
    // tests across modules (themes, skill_layers, chat_history, app_settings,
    // whats_new) all serialize on a single mutex. Cargo's parallel runner
    // would otherwise race separate per-module locks against each other
    // — they each guard their own module's tests but nothing stops two
    // modules from mutating `$HOME` simultaneously. Phase 2 Unit 6's
    // 68-test backfill (commit b65a5195) widened that pre-existing race
    // and broke `whats_new::tests::read_write_clear_state_roundtrips`
    // reliably; centralizing on one lock closes it.
    use crate::themes::HOME_LOCK as TEST_LOCK;

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();
        let s = load();
        assert_eq!(s.default_agent, "claude");
        assert_eq!(s.terminal.font_size, 13);
    }

    #[test]
    fn save_then_load_round_trips() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();
        let mut s = AppSettings::default();
        s.default_agent = "codex".into();
        s.companion.username = "rosson".into();
        save(&s).expect("save");
        let loaded = load();
        assert_eq!(loaded.default_agent, "codex");
        assert_eq!(loaded.companion.username, "rosson");
    }

    #[test]
    fn update_deep_merges_partial() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();
        let mut s = AppSettings::default();
        s.companion.username = "old".into();
        s.companion.ngrok_auth_token = "tok".into();
        save(&s).expect("save");
        let merged = update(serde_json::json!({
            "companion": { "username": "new" }
        }))
        .expect("update");
        // Username updates, ngrok_auth_token preserved by deep merge.
        assert_eq!(merged.companion.username, "new");
        assert_eq!(merged.companion.ngrok_auth_token, "tok");
    }

    #[test]
    fn reset_writes_defaults() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();
        let mut s = AppSettings::default();
        s.default_agent = "codex".into();
        save(&s).expect("save");
        let after = reset().expect("reset");
        assert_eq!(after.default_agent, "claude");
        let loaded = load();
        assert_eq!(loaded.default_agent, "claude");
    }

    #[test]
    fn legacy_projects_key_migrates_in_memory() {
        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();
        ensure_dir();
        let raw = r#"{"projects": {"abc": {"foo": "bar"}}}"#;
        fs::write(settings_file(), raw).expect("write legacy");
        let s = load();
        assert!(s.project_settings.contains_key("abc"));
    }

    /// F3 closure smoke: when `update()` sees `companion.username`
    /// change, it must invoke `crate::companion::invalidate_all_sessions`.
    /// We can't easily start a real companion (needs ngrok + a
    /// listener), so we stuff a fake `Session` into the live
    /// `crate::companion::STATE` and assert that `update()` empties
    /// `state.sessions` for us.
    ///
    /// Pre-Unit-7a this was Tauri's job and the invalidate landed on
    /// an empty in-process state; the daemon-side `STATE` (where the
    /// live sessions actually live) stayed populated. This test pins
    /// down that the call now lands on the daemon-side `STATE` —
    /// which is exactly what closes the F3 gap.
    #[test]
    fn update_with_credential_change_invalidates_companion_sessions() {
        use crate::companion::types::{
            AuthRateLimiter, CompanionState, Session,
        };
        use parking_lot::Mutex as PlMutex;
        use std::collections::HashMap;
        use std::sync::atomic::AtomicBool;

        let _g = TEST_LOCK.lock();
        let _home = HomeGuard::new();

        // Seed disk with a settings.json whose companion.username we
        // can rotate. Disk read happens inside `update()` to compute
        // the old-vs-new diff.
        let mut s = AppSettings::default();
        s.companion.username = "before".into();
        save(&s).expect("save initial");

        // Build a CompanionState with one fake session keyed "tok-A".
        // All fields private to types::CompanionState are public, so
        // we can construct one for the test without test-only helpers.
        let mut sessions = HashMap::new();
        let now = chrono::Utc::now();
        sessions.insert(
            "tok-A".to_string(),
            Session {
                token: "tok-A".to_string(),
                created_at: now,
                expires_at: now + chrono::Duration::hours(24),
                last_active: now,
                remote_addr: "127.0.0.1".to_string(),
                request_count: 0,
                window_start: std::time::Instant::now(),
            },
        );
        let fake_state = CompanionState {
            tunnel_url: PlMutex::new(None),
            sessions: PlMutex::new(sessions),
            ws_clients: PlMutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
            hook_port: 0,
            hook_token: String::new(),
            cors_origins: Vec::new(),
            allow_remote_spawn: false,
            auth_limiter: PlMutex::new(AuthRateLimiter::new()),
            reflow_cache: PlMutex::new(HashMap::new()),
            _tunnel_keepalive: PlMutex::new(None),
        };
        // Install the fake state in the live companion module.
        *crate::companion::STATE.lock() = Some(fake_state);

        // Rotate the username — F3-relevant field.
        let _ = update(serde_json::json!({
            "companion": { "username": "after" }
        }))
        .expect("update");

        // Sessions map should be empty post-update.
        let guard = crate::companion::STATE.lock();
        let state = guard.as_ref().expect("state installed above");
        assert!(
            state.sessions.lock().is_empty(),
            "F3 close: companion.sessions must be empty after a credential rotate"
        );

        // Cleanup so subsequent tests don't see a stale state.
        drop(guard);
        *crate::companion::STATE.lock() = None;
    }

    /// Tiny in-crate tempdir helper — we don't want a top-level
    /// `tempfile` dep for one usage. Mimics the minimal API we need:
    /// auto-create + auto-delete + path accessor.
    mod tempdir_lite {
        use std::path::{Path, PathBuf};

        pub struct TempDir {
            path: PathBuf,
        }

        impl TempDir {
            pub fn new(prefix: &str) -> Self {
                let pid = std::process::id();
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let path =
                    std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}"));
                std::fs::create_dir_all(&path).expect("create tempdir");
                Self { path }
            }

            pub fn path(&self) -> &Path {
                &self.path
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }
}
